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

[章节验收与后续交接](#章节验收与后续交接)汇总本章来源映射；复核入口为 pg_name.rs、pg_session.rs、pg_extended.rs 及独立的 pg_name_test.rs、pg_query_test.rs，仅作静态证据。

[解析承载方案决策](#解析承载方案决策)比较共享 AST 扩展与独立受控 PG 解析入口；取证入口为 yy_parser.rs、parser.y、ast/expressions.rs、ast/dml.rs、ast/lib.rs、[optimize.rs](../pkg/planner/optimize.rs) 与 pg_extended.rs，独立测试入口为 yy_parser_test.rs、pg_extended_test.rs。只核对必要局部交接。

[方言身份传递](#方言身份传递)负责连接到共享内核的身份边界。实际源码入口为 [pg_conn.rs](../pkg/server/pg_conn.rs) 的 `PgService::start` / `negotiate`、[pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::new` / `handle` / `process`、[pg_session.rs](../pkg/server/pg_session.rs) 的 `PgSession` / `SessionQuery::parse`，以及 [conn.rs](../pkg/server/conn.rs) 的 `ClientConn::dispatch`、`SessionDriver::open_ctx` 和 `TiDBContext`。另核对 [driver.rs](../pkg/server/driver.rs) 的 `IDriver::OpenCtx`：它是另一层生命周期抽象，不能冒充 PG 当前调用的 `SessionDriver`。本块不指定尚未取证的内核类型或接口名。

[类型转换语法边界](#类型转换语法边界)的调研入口为 [yy_parser.rs](../pkg/parser/yy_parser.rs)、[expressions.rs](../pkg/parser/ast/expressions.rs) 和上述扩展查询入口；实际共享表达式转换载体另定位到 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `ExprKind::Cast`。只核对本块必要的 AST 与参数接线，不展开类型执行子系统。

[大小写匹配语法边界](#大小写匹配语法边界)的源码入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `PatternLikeOrIlikeExpr`、[parser.y](../pkg/parser/parser.y) 的 `PredicateExpr` / `IlikeOrNotOp` / `LikeOrIlikeEscapeOpt` 与 [pg_name.rs](../pkg/server/pg_name.rs) 的 `rewrite` / `adapt`。语法文件与 Rust 载体分别取证，不以 Go 语义动作推断 Rust 运行接线。

[返回子句语法边界](#返回子句语法边界)的指定入口为 [ast/dml.rs](../pkg/parser/ast/dml.rs)、[parser.y](../pkg/parser/parser.y) 与 [pg_result.rs](../pkg/server/pg_result.rs)；实际共享 DML/投影载体在 [ast/lib.rs](../pkg/parser/ast/lib.rs)，以 `Returning`、`SelectField`、`command` / `encode_formats` 为必要局部交接点。

[冲突子句语法边界](#冲突子句语法边界)沿用 [ast/dml.rs](../pkg/parser/ast/dml.rs)、[parser.y](../pkg/parser/parser.y) 与 [pg_extended.rs](../pkg/server/pg_extended.rs)；实际 INSERT 载体为 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `InsertStmt` / `Assignment`，只补充本块必要的局部取证。

[数组构造语法边界](#数组构造语法边界)的入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `Expr` / `RowExpr` / `ParamMarkerExpr`、[parser.y](../pkg/parser/parser.y) 的 `SimpleExpr` / `ArrayKwdOpt` 和 [pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::process` / `markers`，仅取证构造与类型/参数交接。

[行构造语法边界](#行构造语法边界)的入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `RowExpr` / `Expr::restore`、[parser.y](../pkg/parser/parser.y) 的 `SimpleExpr`、[pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::close` / `scope` / `relation`；独立测试入口为 parser_test.rs 与 pg_name_test.rs，只作静态取证。

[引号与大小写规则](#引号与大小写规则)以 [pg_name.rs](../pkg/server/pg_name.rs) 的名称扫描/改写与 [expressions.rs](../pkg/parser/ast/expressions.rs) 的名称载体为入口；独立测试为 pg_name_test.rs、pg_query_test.rs，仅作静态证据。

[限定名称解析规则](#限定名称解析规则)以 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::relation` / `validate_relations` 与 [ast/dml.rs](../pkg/parser/ast/dml.rs) 为入口，实际共享 TableName 另核对 [ast/lib.rs](../pkg/parser/ast/lib.rs)；独立证据入口为 pg_name_test.rs、pg_query_test.rs。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)以 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::scope` / `relation` 为入口；[ast/dml.rs](../pkg/parser/ast/dml.rs) 为指定导航，实际 CTE 载体在 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `WithClause` / `CommonTableExpression`。独立测试入口为 pg_name_test.rs、parser_test.rs，仅静态阅读。

[模式搜索路径解析契约](#模式搜索路径解析契约)以 [pg_session.rs](../pkg/server/pg_session.rs) 的路径访问器、[pg_name.rs](../pkg/server/pg_name.rs) 的关系门禁和 [pg_catalog.rs](../pkg/server/pg_catalog.rs) 的目录遮蔽接线为入口；独立测试入口为 pg_session_test.rs、pg_query_test.rs，仅静态阅读。

### 目标行为

[章节验收与后续交接](#章节验收与后续交接)以来源四步与完成条件的映射收束本章，给工作流 3 提供解析结构、名称绑定及类型判定边界。

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

#### 引号与大小写规则

本块对应源工作流 2 步骤 3 的名称词法契约，区分名称拼写、绑定键和后端存储约束。静态证据基于提交 `3bd789433dee85dcd9b2037ee631bdcf13a29429`：已读 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Token` / `tokens` / `source_tokens` / `quote` / `rewrite` / `Resolver::relation`。tokens 调用非 native 模式；source_tokens 将未引号名称转为小写 name，双引号名称保留大小写、解码成对双引号，并保存 quoted 及原输入字节 start/end。rewrite 为双引号名称生成反引号文本，关系位置再替换成当前数据库限定名；未被编辑覆盖的文本直接复制。因此 token 中小写 name 不等于整个 SQL 已按 PG 规则折叠，尤其不能据此证明表达式列名/别名已正确绑定。native=true 仅用于 stored_view_select 的存储源提取，不是允许 PG 输入反引号的开关。

当前扫描跳过普通单引号字符串、行注释及普通块注释，保留未编辑原文；未闭合引号/空名称返回 42601。PG 输入反引号、美元引号、可执行 MySQL 注释及嵌套块注释有 0A000 拒绝边界；其中美元字符串与嵌套注释是合法 PG 形式，不能把当前拒绝解释为 PG 语法非法。单引号扫描沿用 native 反斜杠转义，不足以证明 standard_conforming_strings、E 字符串或全部 PG 词法保护正确。错误返回只含 SQLSTATE 和消息，rewrite 返回 String，未见本块原 SQL 到转换后文本的持久位置映射。

已读 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `CiString`、`ColumnName::matches` / `restore` 与 `RestoreCtx::write_name`：original/lowercase 双拼写、按小写匹配及反引号还原不能表达完整 PG 引号敏感绑定，也不保留逐名称 quoted 和原 SQL 跨度。已读 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`（双引号、`"a""b"`、字符串/注释）和 `pg_introspection_names_rejection_boundaries`（空/未闭合引号、`"PUBLIC"` 等拒绝）；[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `postgres_create_table_simple_query` 使用双引号混合大小写表名，但按 schema.name.lower / table.name.lower 查找，未断言大小写不同对象共存或误绑定拒绝。这些仅为静态测试意图，本轮未执行。`pkg/server/doc.go`、`pkg/parser/doc.go`、`pkg/parser/ast/doc.go` 均不存在。

目标在 PG 词法阶段区分标识符、关键字、字符串和注释，保存原拼写、是否引号、解码名称、规范绑定键和原 SQL 跨度；未引号按 PG 规则折叠，双引号保真且 `""` 解码为一个名称字符。resolver 对表、列和别名使用该语义身份，不能统一用 lowercase 或依靠反引号转换制造大小写敏感。共享 AST 扩展与受控 PG 入口的选择交后续解析方案块取证，不预设新接口。若后端存储/目录无法区分 foo 与 Foo，应显式登记能力缺口、阻止静默合并/误读；命名映射或存储改造另行细分，本块不指定方案。限定名、CTE 遮蔽及 search_path 在后续块定义；目录可见性归工作流 6，会话设置归工作流 5。

以下为待新增 `NAME-QUOTING` 用例族，所有运行行为均未验证。成功目标 SQLSTATE 为“不适用（成功）”、无错误位置；类型和元数据按第 1 章分别采集，不把拼写一致当运行通过。MySQL 基线为 8.4，明确记录 sql_mode、lower_case_table_names、对象类别及平台；双引号默认字符串模式与 ANSI_QUOTES 分开验收。

| 输入与正反例/前提 | PG 目标及诊断责任 | MySQL 自身目标及诊断责任 |
| --- | --- | --- |
| 正例：预建 public.foo，列 id=1；分别 `SELECT id FROM foo`、`SELECT id FROM Foo`、`SELECT id FROM "foo"` | 三者均绑定 foo，目标值 1；不将 Foo 作为独立对象。名称折叠发生于词法/绑定交接，非字符串全文转小写。 | 未引号 foo/Foo 的表绑定取决于 lower_case_table_names 与平台，不预定一致；双引号表名在默认模式是负例，ANSI_QUOTES 下按自身表名规则处理，不自动获得 PG 大小写敏感。MySQL 错误码与位置须参考端采集。 |
| 保真正例/误绑定反例：同一 schema 预建 foo(id=1) 和 "Foo"(id=2)；`SELECT id FROM "Foo"` | 目标只返回 2；foo/Foo/"foo" 仍返回 1。再单独以仅有 foo 的夹具查询 "Foo"，应为未定义关系 42P01，不能降级绑定 foo。是否给出 P 及精确位置待 PG 18 参考端取证。 | 不要求其可创建这两个表；必须先按自身配置确认可区分性。以单个表的列名 foo/Foo 做自身不区分大小写正例；不将后端原有规则改成 PG 规则。 |
| 转义正例：预建 `public."a""b"`，id=3；`SELECT id FROM public."a""b"` | 解码名称为 a 后接一个双引号再接 b，目标 3；原拼写与解码值分别保存，不把引号当字符串或分隔两名称。 | 反引号包围 a、双引号、b 为自身标识符正例；ANSI_QUOTES 下同一双引号输入也是名称候选，默认模式该 FROM 输入是负例。结果/诊断独立取证，不把 SQL 文本改写结果当执行证据。 |
| 词法反例：`SELECT * FROM "Foo`；空名称 `SELECT * FROM ""` | 均拒绝目标 42601，不补齐引号、不进入对象查找。解析层保留错误起始引号及输入末尾跨度，P 的准确选择/数值待参考端取证，工作流 7 只传递诊断。 | 同一 FROM 输入在默认模式及 ANSI_QUOTES 下均须拒绝；错误码、阶段、位置独立采集，不套 PG 42601 或 P。自身未闭合反引号名称另作负例。 |
| 方言隔离：反引号包围的 foo 作 FROM 表名；另对比 `SELECT "Foo"` | 反引号输入为 PG 语法负例，目标 42601；当前适配返回 0A000 是已知诊断差异。后者是名称引用，在无 FROM 且无该列时目标 42703，不能返回字符串。 | 反引号表名是自身正例；默认模式 SELECT 双引号 Foo 返回字符串 Foo，ANSI_QUOTES 下是列引用并按自身缺列规则拒绝。隔离回归交错 PG/MySQL 连接，禁止 PG 改写或绑定键泄露。 |
| 保护正例：`SELECT 'Foo "a""b"' /* "Foo" */`，另加行注释中同样名称 | 字符串值保持原大小写及双引号，注释不制造名称/关系节点；目标只返回该字符串。不同 PG 字符串模式与合法嵌套注释须另取证，不据此推广保护集合。 | 普通单引号和普通注释是自身正例，目标相同文本；MySQL 特有可执行注释另按自身规则，不被 PG 扫描门禁改写。 |

位置责任为：解析/适配保留原查询和每次替换区间，后续绑定/解析诊断若基于转换后文本，必须先映射回原 SQL；字符计数与 UTF-8 字节偏移分别处理。PG 的 P 若提供须按原输入从 1 起按字符计数，不能直接使用 Token.start/end 或反引号文本偏移。待新增用例需包含 `"a""b"` 长度变化、public 到当前数据库限定名变化，以及错误前有中文的输入；精确 P、MySQL 自身位置和简单 Q/扩展 Parse 一致性均缺运行证据。合法但尚未支持的形态按明确能力门禁拒绝，不把词法错误、绑定缺失与 0A000 混为同类。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.1.1 [标识符](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-SYNTAX-IDENTIFIERS)、§4.1.2 与 §4.1.5 定义名称、字符串、注释边界；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 提供 42601、42P01、42703 和 0A000 分类；§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P。MySQL **8.4** §11.2 [对象名称](https://dev.mysql.com/doc/refman/8.4/en/identifiers.html) 和 §11.2.3 [名称大小写](https://dev.mysql.com/doc/refman/8.4/en/identifier-case-sensitivity.html) 仅为自身目标依据。U& 引号、编码、名称长度/截断与非 ASCII 折叠的完整契约另需专项官方取证及参考端验证，不由 Rust to_lowercase 推导。18.6 仅沿用来源目标，没有该补丁版本或本地数据库实测结论。

#### 限定名称解析规则

本块对应源工作流 2 步骤 3 的关系名与列/别名限定责任，承接[引号与大小写规则](#引号与大小写规则)。静态证据基于提交 `c05789991a5462588cd20c8c9dedace2f233014f`：已核对 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::relation` / `scope`、`rewrite`、`adapt` 与 `validate_relations`。relation 按点号收集关系分段：单段非 CTE 名在 public 不在路径时返回 42P01；public.t 与当前库 db.public.t 可进入改写，其他 schema、其他库及更多段返回 0A000；无当前数据库时返回 3D000。限定 public 不受 has_public 布尔值限制。adapt 先以空数据库探测，再按需读取 SELECT DATABASE() 并重写：首遍允许三段库名只是探测，不能脱离第二遍当前库检查解释为允许跨库。最终将关系跨度替换为后端当前库和表名，不表示 public 已成为完整 PG schema。

validate_relations 对成功解析的共享 AST 遍历 TableName，拒绝非空且与当前库 lowercase 不同的 Schema（0A000）；解析失败时返回 Ok，后续命令门禁仍须拒绝错误，不能把此 guard 当完整语法/权限检查或保证所有节点可见。指定 [ast/dml.rs](../pkg/parser/ast/dml.rs) 的 `TableName::new` / `restore` 仅有 schema/name 两级及 MySQL 引用还原；实际 guard 使用 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `TableName`（Schema、Name），均不能单独承载 PG 数据库/schema/关系三层、逐段引号与原跨度。scope 只在关系位置调用 relation，a.id 不因此成为 schema.table；列与别名绑定仍需共享解析和绑定层的专项证据，不能由改写成功推定。

已读 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`：t、public.u、db.public.v 的混合 FROM/JOIN、双引号 a.id 及 DML 关系改写；`pg_introspection_names_rejection_boundaries`：other.public.t、other.t、db.private.t、public.t.extra、空当前库和路径无 public 的拒绝断言。二者只调用 rewrite，不覆盖 adapt 的数据库读取或 AST guard。已读 [pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `postgres_create_table_simple_query`，public 的混合大小写建表落到 test 库是局部意图，不是跨库拒绝、别名或完整 schema 的证明；本轮均未运行。目标包 `pkg/server/doc.go`、`pkg/parser/doc.go`、`pkg/parser/ast/doc.go` 不存在。

目标按语法上下文区分关系限定与表达式限定，保留有序分段、每段 quoted/解码名称及原 SQL 跨度。关系一段交给当前数据库内的名称搜索，两段为显式 schema/关系，三段为数据库/schema/关系且数据库必须匹配连接数据库；不得删掉 foreign 前缀后落到同名本地表，也不得用后端 database 名直接接受为 PG schema。resolver 在执行前完成隔离、对象绑定及权限交接；即便跨库用户有后端权限也不能放开 PG 跨库访问。非 public schema 的目标绑定依赖工作流 6 的目录模型，尚未开放时以明确能力门禁 0A000 拒绝，不静默回退 public。CTE 遮蔽由下一块定义；search_path 的完整搜索、状态执行分别由后续名称块和工作流 5 定义，本块只固定查找责任，不展开实现。

下列为待新增 `NAME-QUALIFIED` 用例族，均无当前运行证据。PG 夹具为连接数据库 db、public.t(id=1)、路径包含 public、具备必要权限且无同名 CTE；other 数据库中的同名表取 id=2 以检测误读。成功目标 SQLSTATE 不适用（成功）、无错误位置；结果列类型/元数据按第 1 章另行采集。MySQL 8.4 使用独立夹具和自身权限，默认数据库 db 的 t(id=1)、数据库 public 的 t(id=2)，不能把 PG 的 public 自动映射过去。

| 输入/正反例 | 当前静态边界 | PG 目标与诊断责任 | MySQL 自身目标与诊断责任 |
| --- | --- | --- | --- |
| 正例 `SELECT id FROM t` | public 可搜索且当前库非空时改写到后端 db.t；无 public 时 rewrite 返回 42P01。 | 给定夹具返回 1；未限定名由 resolver 搜索，不硬编码总是 public；路径不含可见 t 时目标 42P01，完整路径规则另块定义。 | 绑定默认库 db.t，目标 1；不读取 PG search_path。 |
| 正例 `SELECT id FROM public.t` | 显式 public 可在 public=false 时改写 db.t；数据库为空返回 3D000。 | 绑定当前库显式 public.t，目标 1，不受 public 是否列入路径影响；对象不存在目标 42P01。 | public 是数据库限定，按独立夹具返回 2；无该库/表或无权限时独立采集自身诊断，不套 PG schema 语义。 |
| 正例 `SELECT id FROM db.public.t` | db 匹配当前库时改写 db.t；不是三个后端名称层次。 | db 必须是连接数据库，目标同 public.t 返回 1。数据库身份在绑定前核对，不能依文本删前缀通过。 | 三段 FROM 关系名是自身语法负例；MySQL 的三段列名 db.t.id 是另一上下文。错误码/位置待参考端采集。 |
| 跨库反例 `SELECT id FROM other.public.t`；另对比 `SELECT id FROM other.t` | 在当前库 db 下均拒绝 0A000。 | 前者拒绝，目标 0A000，不读 other 或本地同名表；后者是当前库 schema other 的关系，不是跨库语法：无该关系目标 42P01，合法但未开放的 schema 按能力门禁 0A000，不伪称 PG 禁止该 schema。 | 前者是三段关系名语法负例；other.t 可作为自身跨库正例，预建且授权时读取 other 库 t，结果与权限独立验收，不受 PG 门禁影响。 |
| 列/别名正例 `SELECT a.id FROM public.t AS a`；反例 `SELECT t.id FROM public.t AS a`；缺列 `SELECT a.missing FROM public.t AS a` | relation 只改 FROM 表；token_scopes 有别名保留意图，无列绑定断言。 | 正例返回 1；显式别名后原表名不再可引用，第二条目标 42P01；第三条目标 42703。binder 按查询作用域处理别名与列，不把 a 当 schema 去跨库查表。 | 用 `FROM db.t AS a` 作自身正例返回 1；原名/缺列负例按自身规则拒绝，诊断独立采集，不套 PG SQLSTATE。 |
| 形态反例 `SELECT * FROM public.`；多段负例 `SELECT * FROM db.public.t.extra` | 尾点缺名称返回 42601；多段关系返回 0A000。 | 不丢空段或截断多余段；前者目标 42601，后者精确 PG 诊断须参考端取证，不能由适配层 0A000 推定。 | 分别按自身关系语法拒绝；多段列引用与关系引用分开，精确错误码/位置待参考端取证。 |

错误责任：parser 负责分段合法性及上下文，resolver/binder 负责数据库隔离、可见关系/别名/列与权限，工作流 7 传递原诊断。所有拒绝必须在读写目标关系前完成，不能借转换后 AST 绕过检查。P 若提供须映射回原 SQL、从 1 起按字符计数；限定名长度变化、逐段引号和错误前中文均纳入待新增位置断言，不能使用改写后的字节偏移。上述目标 SQLSTATE 的精确触发、P 是否提供及数值、错误优先级、简单 Q/扩展 Parse 一致性和双方言交错连接隔离均待运行取证；无当前数据库的 3D000 是现适配边界，连接建立/切库契约由工作流 5、7 另证，不伪造 PG 同场景对照。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §5.10.1 [Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html#DDL-SCHEMAS-CREATE) 定义 schema.table 与只允许当前数据库的 database.schema.table；§5.10.3 将未限定关系交给搜索路径。§7.2.1.2 [表和列别名](https://www.postgresql.org/docs/18/queries-table-expressions.html#QUERIES-TABLE-ALIASES) 定义别名替代原关系名；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 0A000、42601、42P01、42703 分类，具体错误位置仍需参考端补证。§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P；MySQL **8.4** §11.2.2 [Identifier Qualifiers](https://dev.mysql.com/doc/refman/8.4/en/identifier-qualifiers.html) 区分关系的 database.table 与列的 database.table.column。18.6 仅为来源目标，本轮没有该补丁版本或本地数据库实测。

#### 公共表表达式遮蔽规则

本块对应源工作流 2 步骤 3 的 CTE 名称责任，承接[引号与大小写规则](#引号与大小写规则)及[限定名称解析规则](#限定名称解析规则)。静态取证基于提交 `b5ae432d5a2c6682d13b9fe2b3813bd72c766482`：已读 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::scope` / `relation`、`rewrite` / `adapt`。scope 克隆继承的名称集合，在扫描每个定义体后才插入该 CTE 名，再向后续定义和主查询传递；括号递归使用克隆集合，内层新增名称不会泄漏出去。relation 只在单段名命中集合时保留引用；显式 public.t 仍改写到当前后端库的表，即使 public 不在路径中。WITH 后紧接 RECURSIVE 一律返回 0A000，包括没有实际自引用的输入；这只是当前入口门禁，不能称为 PG 的递归规则。

已读独立测试 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`：覆盖 `WITH t AS (SELECT 1) SELECT * FROM t` 在 public=false 时不改写、同名 CTE/public.t 区分及内层 CTE 不泄漏到外层；`pg_introspection_names_rejection_boundaries` 覆盖 RECURSIVE 的 0A000。未执行这些测试；现有断言没有证明嵌套同名的绑定对象、前向引用、自引用、重复定义或错误位置。已读 [ast/dml.rs](../pkg/parser/ast/dml.rs) 的基础 `TableName`，CTE 实际共享载体另定位到 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `WithClause`（IsRecursive、顺序 CTEs）、`CommonTableExpression`（Name、ColNameList、Query、IsRecursive）及 `WithClauseRef`；这些字段存在不等于 PG 绑定接线完整。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_dml_stmt` / `test_set_operator` 注册和 `run_contract_table_with_options`：接受性及顶层节点类型检查不能替代 CTE 作用域断言。`pkg/server/doc.go`、`pkg/parser/doc.go`、`pkg/parser/ast/doc.go` 不存在。

目标由 parser 保留定义顺序、查询块嵌套、递归标记、列名列表及名称的 quoted/规范身份和原 SQL 跨度；resolver/binder 拥有每个查询块的 CTE 名称空间与具体绑定。普通 WITH 的定义体只见先前同级定义及合法外层名称，主查询见本层全部定义；内层同名定义在其可见区域遮蔽外层 CTE，退出内层后恢复外层，不能用无绑定身份的字符串集合证明实现完成。未限定关系先查可见 CTE，再交关系搜索；显式 schema.t 绕过 CTE，按真实关系绑定。非递归定义自身尚未登记时，同名引用仍可绑定合法外层 CTE 或真实表，不能仅凭拼写相同认定递归或无条件拒绝。会话后续语句不保留 CTE 名称，CTE 不成为目录对象；search_path 状态执行及目录可见性分别交工作流 5、6。

PG 的 RECURSIVE 还允许无环前向引用，不等于每个 CTE 都自递归；自引用是否合法由递归项位置等结构约束决定，互递归不属于 PG 已实现能力。本块只登记解析/绑定入口责任，不开放递归执行；能力未开放时保持明确 0A000 门禁，并将参考 PG 的合法结果与 AsterSQL 暂时拒绝分别记录。物化、工作表迭代、优化、SEARCH/CYCLE 与数据修改型 CTE 的执行另行细分，不以本块补建完整 WITH RECURSIVE。

以下为待新增 `NAME-CTE` 用例族，全部运行行为未验证。PG 夹具为当前库 db、可见 public.t(v=9)、不存在关系 a/b/r，普通表搜索路径含 public；MySQL 8.4 独立夹具为默认库 db 的 t(v=9)，显式限定对照使用 db.t，PG 的 public.t 不直接充当 MySQL schema 语义。成功项 SQLSTATE 不适用（成功）、无错误位置；类型、列名及协议元数据按第 1 章另采集。

| 输入/正反例 | PG 目标与当前边界 | MySQL 自身目标 | 诊断责任与证据缺口 |
| --- | --- | --- | --- |
| 基本正例 `WITH t AS (SELECT 1) SELECT * FROM t` | 目标单行 1，遮蔽 public.t 的 9；现有静态测试仅断言 rewrite 保留，public=false 也应命中 CTE。 | 同一 SQL 目标 1，遮蔽默认库真实表。 | binder 绑定 CTE，不能先因搜索路径缺表拒绝；无执行/元数据证据。 |
| 嵌套同名正例 `WITH t AS (SELECT 1 AS v) SELECT t.v, q.v FROM t CROSS JOIN (WITH t AS (SELECT 2 AS v) SELECT v FROM t) AS q` | 目标一行 (1,2)，内层绑定内层 t，外层仍为外层 t。当前集合扫描保留同名引用，不能辨别绑定对象。 | 同一 SQL 目标 (1,2)，分别验证查询块的同名解析。 | binder 记录层级与对象身份；静态改写相同不证明遮蔽正确。 |
| 显式表正例 `WITH t AS (SELECT 1 AS v) SELECT v FROM public.t` | 目标 9；即使移除路径中的 public 仍指显式表，当前 relation 将它改写到 db.t。 | 使用 `WITH t AS (SELECT 1 AS v) SELECT v FROM db.t`，目标 9；db 是数据库限定。 | resolver 绕过 CTE，目录/权限交工作流 6；不存在真实表时 PG 目标 42P01，MySQL 自身诊断另采集。 |
| 顺序正例 `WITH a AS (SELECT 1 AS v), b AS (SELECT v FROM a) SELECT v FROM b`；前向反例 `WITH a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a` | 前者目标 1；后者在无真实 b 时拒绝，目标 42P01，不提前注册未来 b。当前扫描后插入次序吻合局部意图，但不会自行查目录；有真实同名 b 时应另验真实表绑定，不能一概拒绝。 | 前者目标 1；后者同样不可引用后定义的 b，无真实 b 时拒绝，精确码/位置待采集。 | binder 区分未来 CTE 与可见真实表；错误位置应关联定义体中的 b，P 是否提供及数值待证。 |
| 递归关键字边界 `WITH RECURSIVE t AS (SELECT 1) SELECT * FROM t`；无环前向 `WITH RECURSIVE a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a` | PG 合法，目标分别为 1、2；当前适配器两者均在 RECURSIVE 处拒绝 0A000。 | 第一条目标 1；第二条仍不能前向引用，无真实 b 时拒绝；不能复制 PG 放宽规则。 | parser/binder 区分关键字、依赖与实际递归；当前门禁与参考成功分别验收，MySQL 精确诊断待取证。 |
| 自引用边界 `WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM r WHERE n<2) SELECT n FROM r ORDER BY n`；去掉 RECURSIVE 的反例 | 合法形式参考目标两行 1、2，当前入口拒绝 0A000；去掉关键字且无外层/真实 r 时目标 42P01，不自动升级递归。 | 合法形式目标 1、2；去掉关键字且无真实 r 时拒绝，官方示例给出 1146/42S02，仍需此输入实测。 | binder 负责自引用可见性，执行另交工作流 5；不据扫描通过宣称递归可用。 |
| 非法递归反例 `WITH RECURSIVE r(n) AS (SELECT n FROM r UNION ALL SELECT 1) SELECT * FROM r`；同层重复 `WITH t AS (SELECT 1), t AS (SELECT 2) SELECT * FROM t` | 第一条自引用位于非递归项，应拒绝，精确码待参考端取证；第二条应拒绝重复定义，目标 42712。当前分别为 RECURSIVE 门禁与集合去重，后者未显式检查重复。 | 两条分别按自身递归结构与名称唯一性规则拒绝；精确码/位置待采集。 | binder 负责结构及重复名诊断；不能让 HashSet 去重隐藏错误，也不能用 0A000 代替 PG 本来无效输入的真实诊断。 |
| 名称不泄漏反例：先执行基本正例，再单独执行 `SELECT * FROM t`；另将内层 WITH 放入派生表后在外层引用 t | 下一语句/外层均不得使用已结束的 CTE；给定真实表夹具应返回 9，而非 1，删除真实 t 的独立夹具下目标 42P01。现有测试只覆盖内层新增名不泄漏的文本。 | 独立连接/夹具同样绑定真实表 9；无表时按自身诊断拒绝。 | resolver 负责查询块生命周期；跨语句、简单 Q/扩展 Parse 及双方言交错连接均待新增实际断言。 |

诊断按 parser 的形态/跨度、binder 的可见性/定义顺序/重复与递归结构、工作流 7 的错误传递分责。P 如提供，须指向原 SQL，从 1 起按字符计数；定义体引用、第二个重复名及递归项的定位、引号转换和错误前中文均须独立取证，不能用改写后字节偏移。未列精确码的目标需先查官方对应源码或真实参考端，再补矩阵；当前拒绝不是未来功能成功，所有适用断言仍按第 1 章保留“未验证”。

官方依据（2026-10-23 查阅）：PostgreSQL **18** [SELECT 的 WITH/FROM](https://www.postgresql.org/docs/18/sql-select.html#SQL-WITH) 定义普通/递归 WITH 的同级引用顺序、真实表被遮蔽及显式 schema 绕过；§7.8 [WITH Queries](https://www.postgresql.org/docs/18/queries-with.html) 定义单查询作用域及递归项结构。嵌套同名按这些作用域规则制定上述目标，仍需参考端验证具体输入；[18 分支 parse_cte.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/parser/parse_cte.c) 是重复名与递归诊断的后续取证入口，分支可变，不作固定补丁实测证据。[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 42P01/42712/0A000 分类，§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P。MySQL **8.4** §15.2.20 [WITH](https://dev.mysql.com/doc/refman/8.4/en/with.html) 定义查询块搜索、同级仅向前序引用及自递归关键字要求。18.6 仍仅为来源指定目标，没有本轮补丁版本、AsterSQL、PG 或 MySQL 的实测结果。

#### 模式搜索路径解析契约

本块对应源工作流 2 步骤 3 的 schema/search_path 责任，承接[引号与大小写规则](#引号与大小写规则)、[限定名称解析规则](#限定名称解析规则)及[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)。静态证据基于提交 `d0e11420ba7c3e452418375ce494b5769bae811f`：已读 [pg_session.rs](../pkg/server/pg_session.rs) 的 `PgSession::has_public` / `schema` / `public_precedes_catalog` / `execute` 与 `SessionQuery::parse`。当前默认路径仅 public；has_public 只判断成员，schema 只取原始列表首项，public_precedes_catalog 仅在 public 与 pg_catalog 都显式出现且 public 在前时为真。parse 只接受这两个名称、无重复列表及空字符串路径，其他 schema（包括 missing）、重复项返回 0A000；这不是 PostgreSQL 完整路径模型，也没有证明首项是存在且有 USAGE 的有效 schema。

[pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::relation` / `adapt` 将未限定非 CTE 关系交给 has_public 门禁；public 缺席返回 42P01，显式 public.t 绕过该门禁并改写到当前后端库，非 public/跨库拒绝沿前块契约。[pg_catalog.rs](../pkg/server/pg_catalog.rs) 的 `CatalogQuery::parse_session_with_types` 在 public_precedes_catalog 为真时，通过当前库快照检查隐式目录关系是否被同名用户表遮蔽，再交给有界目录解析；`current_schema` 字段取自 session.schema()。目录行生成中 pg_namespace 仅映射当前库为 public，并提供 pg_catalog，不因路径排除而删除目录行。上述接线不是任意 schema 的 resolver，也不能由路径成员推导对象存在或用户可见。

独立测试只阅读、未执行：[pg_session_test.rs](../pkg/server/pg_session_test.rs) 的 `pg_introspection_namespace_parser_boundaries` 覆盖有界 SET/RESET/SHOW 接受与缺失 schema、重复名拒绝；[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `pg_introspection_namespace_session_state` 有两连接不同路径、空路径 current_schema 为 NULL、隐式 catalog 可查询、RESET 和 Parse 后 SET 的 current_schema 断言；`pg_introspection_names_live_tables` 有路径仅 pg_catalog 时未限定用户表 42P01、显式 public 表仍读取的断言。未据此声称同名 catalog 完整优先级、权限过滤或缓存重绑定已通过。`pkg/server/doc.go` 不存在，本块源码和测试阅读均不替代当前运行证据。

目标由工作流 5 拥有连接私有的路径设置、SHOW、RESET、SET LOCAL 及事务恢复；本块只定义 resolver 消费状态的责任，不指定新类型或方法。交接信息须能区分原始设置与有效有序 schema、连接数据库/角色、名称身份与对象类别，并关联工作流 6 提供的真实对象、schema 存在性及权限可见性；不能用 has_public 或首项字符串取代目录查询。未限定关系先按前块查可见 CTE，再按有效关系搜索路径查首个匹配对象；显式 schema.t 绕过 CTE 和路径成员筛选，仍须执行目录存在性、权限及当前数据库隔离检查，失败不能静默回落 public。

PG 目标规则：省略 pg_catalog 时隐式优先于显式路径；显式列出则遵从所在位置。空显式路径不禁用隐式 catalog，普通 public 表不能因此可见；存在临时 schema 时，关系/类型还须按 PG 的临时 schema 优先级处理，不能将关系顺序直接用于函数/运算符。不存在或无 USAGE 的路径项按官方规则忽略，特殊项 "$user" 按当前用户解析；这些完整规则是工作流 5、6 的后续依赖，当前入口不支持任意 schema、用户路径或临时 schema 的部分须显式登记门禁 0A000，不能误称 PG 要求拒绝。对象创建默认 schema、函数/运算符重载及临时对象实现不在本块展开。current_schema 的目标是首个有效显式 schema（没有则 SQL NULL），不能因隐式 pg_catalog 总可搜索便返回 pg_catalog；SHOW 原值、有效路径及目录可见对象分别验收。

以下为待新增验收入口，所有运行行为均“未验证”。夹具：同一 PG 数据库仅有 public.t(id) 一行 1，未建临时对象，角色具有所需 USAGE/SELECT；目录同名场景另建 public.pg_namespace(nspname) 一行 'user_marker'，参考端 pg_catalog.pg_namespace 含 pg_catalog 行。MySQL 独立默认库 db 中准备 t(id)=1，仅按自身名称规则对照，不使用 PG 设置。

| 场景与正反例 | PG 目标结果与当前边界 | MySQL 适用性与目标 | SQLSTATE、定位及交接 |
| --- | --- | --- | --- |
| public 在路径：`SET search_path TO public` 后 `SELECT id FROM t`；反例改为不存在的 missing_t | 返回 1；隐式 catalog 仍先搜索。missing_t 目标 42P01，不能以路径含 public 作为存在证明。 | `SELECT id FROM t` 在 db 返回 1；PG SET 不适用。缺表按自身诊断取证。 | 成功不适用；缺表由 resolver 诊断原关系名，工作流 6 负责对象存在性。 |
| public 不在路径：`SET search_path TO pg_catalog` 后分别 `SELECT id FROM t`、`SELECT id FROM public.t` | 前者目标 42P01；后者仍返回 1。当前有对应有界测试意图，显式名不受路径成员限制不等于免权限检查。 | t 仍绑定默认库；对应显式写法为 db.t。public.t 在 MySQL 是数据库限定，不能直接当 PG schema 对照。 | resolver 对未找到的 t 定位，显式名按前块权限/目录交接；不得转而读取其他数据库。 |
| catalog 同名与顺序：分别设置 public、pg_catalog, public、public, pg_catalog，执行 `SELECT nspname FROM pg_namespace WHERE nspname = 'pg_catalog'`；再显式查 pg_catalog.pg_namespace 与 public.pg_namespace | 前两种隐式查询返回 pg_catalog 行；第三种绑定用户表返回零行。显式 catalog 查询总返回该行；显式 public 查询在此谓词下零行。顺序控制的是对象绑定，不能靠 UNION 两个对象模拟。 | PG 路径和目录 SQL 不适用；旁路 `SELECT id FROM db.t` 始终返回 1，不受 PG 连接设置影响。 | 各成功项 SQLSTATE/位置不适用；同名夹具、绑定身份、列元数据及隐式/显式一致性由工作流 6 协作验收，当前未验证。 |
| 空路径：`SET search_path TO ''` 后查询 t、public.t、pg_catalog.pg_namespace，以及 `SELECT current_schema()` | t 目标 42P01；public.t 返回 1；显式 catalog 可查，隐式 pg_namespace 也仍绑定 catalog；current_schema 目标 SQL NULL。空路径不等于禁用所有对象。 | PG SET/current_schema/catalog 不适用；旁路查询 db.t 返回 1。 | 缺表归 resolver；空路径设置和内省值归工作流 5，目录对象归工作流 6；其余成功无错误位置。 |
| 两 PG 会话 A=public、B=pg_catalog，交错查询 t/public.t，随后仅 A 改为空路径；MySQL M 同时查 db.t | 初始 A 的 t 返回 1，B 的 t 为 42P01，两者 public.t 都返回 1；A 改路径后仅 A 的未限定查找改变，B 状态保持。简单 Q 与扩展查询分别采证。 | M 始终返回 1；不向 M 注入 PG SET 语句并忽略错误。 | 工作流 5 拥有状态隔离，resolver 消费所属连接的有效状态；不能从 current_schema 的两连接测试推广为全链路绑定隔离。 |
| 能力边界反例：`SET search_path TO missing, public`、`SET search_path TO "$user", public` | PG 参考端按存在性/USAGE 展开或忽略相应项；当前入口返回 0A000，作为有界能力差异登记，不作为 PG 语法错误或未来成功证据。 | PG 路径语句不适用，MySQL 自身默认库逻辑独立。 | 本轮不猜造任意角色/权限组合结果；需工作流 5、6 的夹具和参考端证据后细分开放条件。 |

诊断责任分开：路径设置语法与拒绝归工作流 5，名称无匹配的 42P01 归 resolver，目录/权限诊断归工作流 6，错误传输归工作流 7。0A000 为当前有界能力门禁，不能替代 PG 合法输入的完整目标。P 如提供须指向原 SQL，从 1 起按字符计数；双引号、错误前中文和改写跨度均待独立取证，不从后端改写文本计算。Parse/Bind/Execute 间改变路径时，既有 current_schema 断言不能决定关系绑定时点、重新分析及缓存失效规则；须后续与工作流 5 单独取证再定义，不承诺所有已准备语句固定或立即重绑定。类型、OID、NULL 元数据、精确诊断位置及参考端结果均须按第 1 章补证。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §5.10.3/5.10.5 [Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html) 说明路径和系统目录；§19.11.1 [search_path](https://www.postgresql.org/docs/18/runtime-config-client.html#GUC-SEARCH-PATH) 给出隐式/显式 catalog、有效路径、临时 schema 及用户项规则；§9.27.1 [Session Information Functions](https://www.postgresql.org/docs/18/functions-info.html#FUNCTIONS-INFO-SESSION) 定义 current_schema/current_schemas。[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 42P01/0A000 分类，§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P。MySQL **8.4** §11.2.2 [Identifier Qualifiers](https://dev.mysql.com/doc/refman/8.4/en/identifier-qualifiers.html) 支持上述默认库/数据库限定对照。18.6 仅为来源指定目标，本轮未运行该补丁版本、AsterSQL、PG 或 MySQL，不将大版本规则当作补丁实测。

#### 解析承载方案决策

本块对应源工作流 2 步骤 4，汇总此前六类语法和名称规则，决策为：**先做最小语法原型，再选择最终接入方式**。目前只确定共享规范执行内核、连接私有方言身份、原 SQL 保真及显式能力门禁；不认定共享 AST 扩展或独立受控 PG parser 已胜出。候选 B 的“独立”仅指词法/语法入口和必要的语法表示，不包含第二套 resolver、优化器、执行器或存储。现有文本适配保留为有界过渡，不作为第三条通用翻译路线。

证据基于提交 `a2d37b4477ba9feb4492d32c780a38b2560f64ff`，本轮仅阅读源码和独立测试。已核对 [yy_parser.rs](../pkg/parser/yy_parser.rs) 的 `Parser::ParseSQL` / `ParseOneStmt` / `yySetOffset`：Scanner/goyacc 决定接受和诊断，入口返回共享 Node，单语句入口检查数量，偏移写入不等于完整字符跨度映射。[parser.y](../pkg/parser/parser.y) 有 Go 语义动作的 `IlikeOrNotOp`、`ReturningClause`、`OnDuplicateKeyUpdate` 及 RowExpr 分支，不能作为 Rust PG 接线证明。[expressions.rs](../pkg/parser/ast/expressions.rs) 的 `PatternLikeOrIlikeExpr` 以 u8 保存 escape，`RowExpr` 只列 values；[dml.rs](../pkg/parser/ast/dml.rs) 是基础类型入口，其 `TableName`/恢复逻辑不能代替 PG 分段引号身份。实际共享 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `ExprKind::Cast` 使用 FieldType，`InsertStmt` 有 OnDuplicate 赋值列表和 Returning 投影，却不足以表达 PG 冲突目标及两类谓词；候选扩展仍须逐消费者取证。

[optimize.rs](../pkg/planner/optimize.rs) 的 `Optimize`、`OptimizeAstNode`、`OptimizeAstNodeNoCache` 接收 `resolve::NodeW` 及共享会话/InfoSchema；Optimize 区分 EXECUTE 与普通缓存路径。这是共享规划边界证据，不证明任意新 PG 语法可直接交给它，更不能通过绕开正常入口跳过只读准入、缓存或成功后效应。[pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::process` 对普通 Parse 依次执行名称/SQL 适配、`markers`、结果适配并调用 `prepare_statement`，会话/目录分支另行处理；普通参数 OID 缺省/为零时仍返回 0A000。这里没有已取证的接收 PG AST 的准备接口。`markers` 的重复/乱序参数映射也不是原查询诊断映射。

独立测试已读 [yy_parser_test.rs](../pkg/parser/yy_parser_test.rs) 的 `failure_returns_warnings_and_parser_reuse_clears_them`、`hint_uses_live_scanner_mode_and_position`（警告生命周期、scanner mode/位置），以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 `marker_mapping_preserves_literals_comments_and_index_order`、`canonical_prepare_parameter_syntax_probe`（词法保护、参数顺序及共享准备入口的语法边界断言意图）。它们没有覆盖本块两条路线的端到端保真，且本轮未执行。`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/planner/doc.go`、`pkg/server/doc.go` 不存在；不把模块说明或历史通过记录当作当前行为证据。

| 比较维度 | 候选 A：扩展共享 AST / 现有解析链 | 候选 B：独立受控 PG 解析入口，再交共享内核 |
| --- | --- | --- |
| 接线成本 | 可复用 ParseSQL、Node 与既有访问链；仍需连接方言接入、Rust 规约动作及缺失字段，不能只改 parser.y。 | 需新增有限 PG 词法/语法及保真转换，并取证共享准备入口如何接收节点；当前 prepare_statement 只见 SQL，接口成本未知。 |
| 共享内核复用 | 载体较近，但每个新增字段仍须穿过 visitor、resolver、planner、Describe；字段存在不等于消费者支持。 | 必须转换到共享表示及正常规划入口；无法表达的语义先明确拒绝，不能建 PG 专用执行内核绕过。 |
| MySQL 回归面 | 修改共享词法、优先级、SQL mode 或 AST 恢复可能影响现有入口；ILIKE/RETURNING 已有扩展候选，须先记录真实 MySQL 基线，不能直接删去。 | 独立入口可隔离语法接受面，但共享表示/消费者与缓存仍可能回归；必须验证交错连接和重连，不能据入口独立即宣称隔离。 |
| 源码跨度/错误定位 | yySetOffset 是起点，仍需补原拼写、quoted、逐节点跨度和改写映射；现有 near/line 不能冒充 PG P。 | 可从原 SQL 记录跨度，但转换及内核诊断必须回溯原查询；字节与字符位置、多字节前缀及 EOF 都要验证。 |
| 参数绑定 | 复用现有准备路径需保留 `$n` 身份和出现顺序，不能让每个 `?` 变成独立 PG 参数；OID 推断仍为缺口。 | PG 参数节点可保留编号，但转换/准备元数据须证明重复绑定一致；不能用参数值拼 SQL 或暗中沿用文本替换。 |
| 维护成本 | 一套主要语法链，但双方言分支与字段语义须持续回归；受共享语法限制的成本尚未测量。 | 增加语法与转换层的双向维护、版本跟踪及拒绝集合；必须逐能力登记，不能把外部 parser 的接受范围当成支持集合。 |

六类语法的最小保真清单沿用对应小节，不重定语义：`::` 保留转换链、目标名/修饰及参数；ILIKE 保留操作类别、NOT、ESCAPE 出现性/表达式和优先级；RETURNING 保留三类 DML 的有序投影/别名及 Describe 交接；ON CONFLICT 保留省略/推断/约束目标、两种动作、两类谓词及 excluded 绑定；ARRAY 保留构造类别、维度嵌套与空列表；ROW 保留显式性、字段顺序/嵌套并区分分组和子查询。另须携带标识符分段、原拼写/引号、CTE 定义顺序和查询块作用域，供 resolver 消费；schema/search_path 的状态执行归工作流 5、目录可见性归工作流 6。不得把这些信息提前压成 MySQL SQL 后再声称仍然保真。

后续原型是待新建的独立任务，限定同一组原始 SQL、连接身份、编码、显式参数 OID 与参数编号作为输入，两候选使用相同用例清单。候选接口只描述数据交接，不命名新 API：A 从带连接身份的解析入口产生扩展共享节点；B 从受控 PG 入口产生保真语法表示，再转换为共享节点。两者均输出原 SQL/节点跨度、参数身份、诊断及共享消费者交接结果。先取证节点到 `resolve::NodeW` 和准备元数据的必要局部接线，不能假定现有 prepare_statement 接收 AST；本轮不实现此原型，也不生成接口定义。

下表是原型最小输入集，完整行为用例仍引用前面小节，所有树形、绑定、SQLSTATE 与位置均未实测。成功的语法项 SQLSTATE 为“不适用（成功）”，不代表可执行；涉及数据修改的输入只解析/描述，不发出写入。

| 输入、正反例 | PG 目标及责任 | MySQL 对照及证据边界 |
| --- | --- | --- |
| `SELECT ($2)::int4, ($1)::int4, ($2)::int4`，参数显式 int4；反例 `SELECT 1::` | 正例有三个独立转换节点及编号序列 2/1/2；反例目标 42601，parser 定位原输入末尾，工作流 7 传输 P。精确 P 待参考端采集。 | `::` 是隔离负例；自身正例用三个 CAST(? AS SIGNED)，绑定顺序 2/1/2。MySQL 精确诊断码/位置仍待取证。 |
| `SELECT 'Ab' ILIKE 'a%' ESCAPE '!'`；`SELECT ':: $1 ILIKE' /* $2 */` | 前者保留谓词与显式 ESCAPE；后者只有字符串和注释，不制造参数或转换。匹配值/locale 交工作流 4，成功无错误位置。 | 前者是目标隔离负例，但现有共享扩展基线须先核实；后者为自身字符串正例。不能用 LIKE 替换证明 PG 语义。 |
| `INSERT INTO t(id) VALUES ($1) ON CONFLICT (id) DO NOTHING RETURNING id`；反例尾部只有 RETURNING | 保留冲突目标、动作及投影；缺投影目标 42601，定位 EOF。绑定需后续 t 夹具，写入/返回行交工作流 5；合法未开放路径目标先于副作用拒绝 0A000。 | PG 子句为目标负例；自身 INSERT/ON DUPLICATE KEY UPDATE 用例单独验收，不认为动作等价。本地拒绝码/位置仍缺证。 |
| `SELECT ARRAY[1,2], ROW(1,2), (1)`；反例 `SELECT ROW(1,` | 区分数组、两字段行与标量分组；未闭合输入目标 42601，精确末尾位置待采集。数组/record 类型及编码不由原型补齐。 | ARRAY 构造不作为自身正例；ROW/分组按自身合法上下文检查，不能要求 MySQL 返回 PG 复合值。精确诊断待双方参考取证。 |

以上诊断输入再加入合法多字节字符串/注释前缀、带引号分段名和嵌套 WITH，核对跨度及作用域保真，不扩写执行功能。原型成功标准是：两路线分别给出这些输入的树形/拒绝、原 SQL 字符定位、参数映射及共享消费者最小交接证据，并记录修改面、不能承载字段和维护代价；双方言基线、并发连接身份及缓存隔离须有独立证据。可暂以明确门禁拒绝未开放合法语法，但拒绝本身不能算该语法交接成功。若必须丢信息、通用文本替换、用全局可变方言开关、执行写入获取 Describe 或补建第二套执行内核，该路线淘汰；若只是消费者尚未支持，应记录共同缺口和下一项局部取证，不能凭两路线均未编译/零测试选胜者。证据仍不足时保持未决，明确缺少哪个输入/交接结果，不递归补建子系统；原型完成后单独评审承载决策，再按能力细分实现。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.2 [Value Expressions](https://www.postgresql.org/docs/18/sql-expressions.html) 的参数、转换、数组与行分类，以及 [INSERT](https://www.postgresql.org/docs/18/sql-insert.html) 的冲突/返回子句；协议 §54.2.3 [Extended Query](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-EXT-QUERY) 区分 Parse、Bind 与 Execute，§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 规定 P 为原查询从 1 起的字符位置；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 区分 42601 与 0A000。其他语法及 MySQL 规则沿用前面各块的官方来源和缺证说明。18.6 仅为来源指定目标，本轮没有该补丁版本或真实数据库的运行证据。

#### 章节验收与后续交接

本块累计审阅本章十二个前序文档块，对应来源任务四个步骤及完成条件；只确认文档契约可交接。前序交付可由提交 `a19b8cba62` 至 `00c684fb49` 的十二条文档提交和本章小节核对，不依赖已删除的编号任务文件。方案结论仍是先做最小原型，未选定最终承载方式；所有功能和双方言行为仍为未验证。

下表将来源步骤映射到小节、后续独立实施边界与验收入口。入口中的用例族均待新增或细化，沿对应小节的 PG/MySQL 正反例、夹具、目标结果与诊断责任登记，不代表现有通过测试；本表不重复 SQL，也不展开完整 parser 实施清单。

| 来源需求 | 本章小节及交接内容 | 后续独立实施块 | 验收入口与所需证据 |
| --- | --- | --- | --- |
| 步骤 1：连接私有身份链 | [方言身份传递](#方言身份传递)：listener→session→parser→resolver→planner→expression context | 先取证身份拥有权与关闭/重连，再逐交接点细分 | 待新增 DIALECT-IDENTITY 三类场景；简单/扩展查询、两 PG 连接与 MySQL 交错、缓存隔离均需当前证据，不能只比 SELECT 1 的值。 |
| 步骤 2：`::` | [类型转换语法边界](#类型转换语法边界)：转换链、类型名称与参数来源 | 单独取证转换节点保真，类型可行性交工作流 3 | 待新增该小节的优先级/嵌套、重复参数、缺类型与词法保护断言；区分树形、类型失败及原 SQL 位置。 |
| 步骤 2：ILIKE | [大小写匹配语法边界](#大小写匹配语法边界)：操作类别、否定与 ESCAPE | 单独细分匹配语法，求值交工作流 3、4 | 待新增该小节的谓词/NOT/优先级/ESCAPE 正反例；先采集 MySQL 既有扩展基线，不能将 LIKE 当等价翻译。 |
| 步骤 2：RETURNING | [返回子句语法边界](#返回子句语法边界)：三类 DML 的投影与 Describe 交接 | 单独细分投影保真与描述接线，副作用交工作流 5 | 待新增三类 DML、零行仍有列、空投影/错位拒绝；Describe 不写入，工作流 3、7 分别验证类型和元数据，0A000 门禁不算功能成功。 |
| 步骤 2：ON CONFLICT | [冲突子句语法边界](#冲突子句语法边界)：目标、动作、双谓词与 excluded | 单独细分冲突语法及绑定，执行交工作流 5 | 待新增 CONFLICT-SYNTAX；省略/推断/约束目标及反例逐项检查，不以 MySQL 冲突更新代替 PG 契约。 |
| 步骤 2：数组构造 | [数组构造语法边界](#数组构造语法边界)：构造类别、嵌套与空列表 | 单独细分数组树形，定型/形状与编码交工作流 3、7 | 待新增 ARRAY-SYNTAX；区分语法完整、空数组定型、混合类型及非矩形拒绝，不把数组改成行或 JSON。 |
| 步骤 2：行构造 | [行构造语法边界](#行构造语法边界)：显式/隐式行、标量分组与嵌套 | 单独细分行/分组保真，复合类型交工作流 3 | 待新增 ROW-SYNTAX；ROW(1,2)、(1,2)、(1)、ROW(1, 分别验收树形、定型/拒绝与执行边界，不强求 MySQL 可返回 PG record。 |
| 步骤 3：引号与大小写 | [引号与大小写规则](#引号与大小写规则)：原拼写、quoted 与绑定身份 | 单独细分名称保真与精确绑定，后端可区分性另取证 | 待新增 NAME-QUOTING；foo/Foo/"Foo"、双引号转义及缺对象反例，MySQL 配置分别记录，不能让 lowercase 合并对象。 |
| 步骤 3：限定名 | [限定名称解析规则](#限定名称解析规则)：关系分段、列/别名及当前数据库隔离 | 单独细分关系限定与列作用域，目录协作工作流 6 | 待新增 NAME-QUALIFIED；t/public.t/db.public.t、跨库拒绝及 a.id，不截断前缀后误读本地表。 |
| 步骤 3：CTE 遮蔽 | [公共表表达式遮蔽规则](#公共表表达式遮蔽规则)：定义顺序、查询块与绑定对象 | 单独细分普通 CTE 绑定与递归门禁，不补建递归执行 | 待新增 NAME-CTE；嵌套同名、显式表绕过、前向/自引用/重复名及退出作用域，字符串集合保留不等于绑定通过。 |
| 步骤 3：schema/search_path | [模式搜索路径解析契约](#模式搜索路径解析契约)：有效有序路径与显式名 | 单独细分 resolver 消费路径，状态/目录分别交工作流 5、6 | 待新增该小节的 public 在/不在、catalog 顺序、空路径与两会话隔离；绑定时点及缓存重分析另取参考端证据。 |
| 步骤 4：两条方案比较 | [解析承载方案决策](#解析承载方案决策)：共享 AST 扩展和受控 PG 入口 | 独立最小原型取证后复审，不新增第二套执行内核 | 待新增该小节的同输入候选对照；按六个维度审查树形、参数、原 SQL 跨度与共享消费者交接，缺证保持未决。 |
| 完成条件：双方言示例、错误位置及隔离 | 本表各小节及[方言身份传递](#方言身份传递)的交错连接用例 | 单独细分原 SQL 诊断与双方言回归；扩展加载继续排除 | 待新增中文前缀、引号/限定名改写、EOF 和 Q/扩展 Parse 的位置断言；PG SQLSTATE/P 与 MySQL 自身诊断分别采证，不能把改写后的字节偏移当字符位置。 |

本轮复核源码的基线为 `00c684fb49`：`pg_name.rs::Resolver::scope` / `relation`、`rewrite` / `adapt` / `validate_relations` 仍是有界名称扫描、文本改写及成功解析 AST 的跨库守卫；`PgSession` 只保存局部路径及失败事务状态，`has_public` / `schema` 不能替代有效路径和目录模型；`Extended::process` 的普通 Parse 仍先适配和 markers，再向 `prepare_statement` 传 SQL，未知参数 OID 返回 0A000。未见上述入口提供完整 PG 语法表示、全链身份或原 SQL 持久位置映射，不从这些局部事实推断所有其他入口均不支持。

本轮实际阅读独立测试 `pg_name_test.rs::pg_introspection_names_token_scopes` / `pg_introspection_names_rejection_boundaries`，以及 `pg_query_test.rs::pg_introspection_namespace_session_state` / `pg_introspection_names_live_tables`：前两者断言 rewrite 的文本/拒绝，后两者有两连接局部路径、current_schema 及 Q/扩展查询名称适配的断言意图。它们没有替代本表逐能力的树形、绑定对象、精确错误位置和 PG/MySQL 全链路隔离验收；本轮未执行这些测试。`pkg/server/doc.go` 不存在，模块说明只作导航。

交给工作流 3 的只读输入是本章及第 1 章的矩阵/证据/用例格式：解析结构须保留操作类别、有序表达式、转换目标、数组/行嵌套、参数编号及原 SQL 跨度；名称绑定须提供分段/引号身份、CTE/别名作用域、连接数据库及有效名称环境；类型阶段再确定类型名与重载、共同元素类型、空数组定型、行字段类型、参数推断、转换/NULL 和结果元数据，不由语法接受替代类型判定。本章未证明这些交接接口已存在，也不命名新 API；工作流 3 应逐能力补齐类型目标和未知诊断，不接管 parser 完整实现。

跨工作流责任保持独立：工作流 5 拥有 search_path 设置/事务恢复、DML 副作用与递归执行；工作流 6 拥有 schema/对象/权限可见性及目录身份；工作流 7 拥有参数/结果编码、Describe 协作、SQLSTATE/P 传输与协议恢复。本章只约束交接，函数/运算符求值归工作流 4，客户端共存证据汇入工作流 8。本轮不执行这些工作流，也不创建运行器、数据库环境、代码或原型。

累计文档验收条件是：本章恰有十三个四级块且五个责任索引可达；上述来源四步和完成条件无遗漏；每个语法/名称规则均能定位模块、目标、范围外、验收与单独细分入口；正反例分方言写明前提、目标、错误责任及缺证状态；两方案维持最小原型取证门禁，工作流 5/6/7 未被吞并。本轮完成仅表示这些文档条件满足。全链路身份/缓存隔离、语法树保真、名称绑定、类型/OID、执行与副作用、精确 SQLSTATE/P、真实客户端/双方言差分及性能均未验证；未知错误码、是否提供 P 及其数值先补官方来源或参考端取证，禁止猜造。

本块没有新增外部行为规则；累计目标沿用各小节的具体官方来源。2026-10-23 复核 PostgreSQL **18** §4.2 [Value Expressions](https://www.postgresql.org/docs/18/sql-expressions.html) 的语法/类型交接分类、§5.10 [Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html) 的名称责任和 §54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 的原查询字符位置要求。其他规则与 MySQL 8.4 对照保留各块的官方依据及缺证说明；18.6 仅为来源目标，没有该补丁版本实测证据。

### 范围外

[章节验收与后续交接](#章节验收与后续交接)不增加能力设计、代码、原型或完整 parser 实施清单，不执行工作流 3；工作流 5/6/7 的状态、目录和协议责任独立交接。

[解析承载方案决策](#解析承载方案决策)本轮不写原型、API、parser/AST 或执行代码，不增加依赖、平台、数据库环境或性能基准；会话、类型、目录、协议及执行实现继续由相应工作流独立细分。

[方言身份传递](#方言身份传递)本轮不实现方言类型、解析原型、共享 AST、缓存接线或运行测试平台；不扩建 resolver、planner、表达式子系统。六类语法、引号/大小写、限定名、CTE 及 schema/search_path 名称规则由后续文档块定义；会话设置执行归工作流 5，目录实现归工作流 6，错误传输与恢复协作工作流 7。现有文本适配只作为有界入口证据，不承担通用 SQL 翻译方案，也不据其推断完整 PG 支持。

[类型转换语法边界](#类型转换语法边界)不实现 PG parser、转换执行、类型推断或元数据；不扩建类型目录、数组/行子系统。转换语义归工作流 3，协议 OID、错误传输与恢复协作工作流 7。

[大小写匹配语法边界](#大小写匹配语法边界)不实现 parser/AST 接线、匹配算法、locale/collation、NULL、重载或错误响应；求值语义交工作流 3、4，协议诊断交工作流 7，不扩展其他匹配家族。

[返回子句语法边界](#返回子句语法边界)不实现解析器、共享 AST 接线、投影求值、事务或 wire 编码；真实写入/返回行交工作流 5，类型和结果描述协作工作流 3、7。ON CONFLICT 本身及 OLD/NEW、MERGE 等组合不由本块扩写实现。

[冲突子句语法边界](#冲突子句语法边界)不实现 upsert、唯一性/索引推断、锁、并发、更新求值和事务；执行与副作用交工作流 5，名称/类型协作本章后续规则及工作流 3，错误响应交工作流 7。

[数组构造语法边界](#数组构造语法边界)不实现数组类型、下标、运算符、存储或 wire codec；ARRAY(子查询) 是另一个合法构造形式，留待单独定义支持集合，不能当作本块已覆盖。类型、转换及 OID 归工作流 3，参数/结果编码归工作流 7。

[行构造语法边界](#行构造语法边界)不实现 parser/AST、复合类型、行比较、NULL 求值或协议编码，不补建关系分组解析；类型与转换交工作流 3，比较/NULL 交工作流 3、4，元数据和错误响应交工作流 7。

[引号与大小写规则](#引号与大小写规则)不实现词法器、名称绑定、存储大小写映射或错误位置接线，不扩建目录、CTE 或 search_path；后端名称区分能力作为独立缺口交接，不以文本改写代替。

[限定名称解析规则](#限定名称解析规则)不实现 parser/AST、名称绑定、权限、跨库访问或完整 schema 模型；CTE/search_path 交后续块，会话状态和目录实现分别归工作流 5、6，协议诊断归工作流 7。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)不实现解析/绑定、递归执行、物化或优化，不扩建目录及 search_path；只固定名称可见性与入口门禁责任，执行协作工作流 5、目录协作工作流 6、诊断协作工作流 7。

[模式搜索路径解析契约](#模式搜索路径解析契约)不实现路径状态、RESET、事务恢复、目录对象或权限可见性；工作流 5、6 分别主责。只约束 resolver 消费顺序、隐式 catalog、空路径和限定名的交接，不承诺任意 schema 已可用。

### 验收

[章节验收与后续交接](#章节验收与后续交接)按本块累计条件检查十三个文档块、来源映射和双方言诊断责任；源码/测试阅读及文档完成均不能升级为功能通过。

[解析承载方案决策](#解析承载方案决策)的文档验收为：六个比较维度均有源码依据及缺证限制，六类语法信息齐全；已选择先做最小原型，输入、候选数据交接、成功/淘汰标准和后续任务边界明确。PG/MySQL 正反例、SQLSTATE/原 SQL 定位责任与未验证状态分开记录；不以候选节点存在或静态测试阅读声称方案已可用。

[方言身份传递](#方言身份传递)的文档通过条件：六段责任链均区分实际入口、状态拥有者/生命周期、目标与缺口；简单/扩展查询身份一致；PG/MySQL 同时连接及两 PG 连接交错场景可单独拆分，负例、错误责任与证据状态明确；局部测试阅读不升级为全链路支持。当前仅文档契约交付；parser 内部接线、共享名称绑定、规划/缓存、表达式上下文、错误位置及真实双方言隔离行为全部未验证，实施后须按第 1 章采集有效证据。

[类型转换语法边界](#类型转换语法边界)的文档验收为：上述输入形态、优先级/嵌套、参数位置、缺类型反例及词法保护均有双方言目标、诊断责任和证据状态；AST 候选与保真缺口明确。现有测试阅读不替代新增树形/位置断言及实际双方言对照，所有行为仍未验证。

[大小写匹配语法边界](#大小写匹配语法边界)的文档验收为：操作符类别、左右操作数、否定、ESCAPE 显式性与保真、优先级及不完整输入均有双方言目标和诊断责任；当前源码、测试意图与运行证据区分清楚。全部运行行为仍未验证，不能以共享语法或同值结果代替 PG 树形与双方言隔离证据。

[返回子句语法边界](#返回子句语法边界)的文档验收为：三种 DML、表达式/星号/别名、空投影与错位反例、零行和词法保护均有双方言目标与诊断责任；解析树、返回列元数据、真实副作用分开验收。现有非空 Returning 的 0A000 门禁不得被当作功能成功，全部运行行为未验证。

[冲突子句语法边界](#冲突子句语法边界)的文档验收为：目标的三种形态、两种动作、两类谓词、excluded 与目标别名、缺动作/缺目标反例均有保真和诊断责任；MySQL 对照、参数/词法保护及执行交接明确。所有运行行为未验证，不能将普通 INSERT 测试或文本改写当作冲突行为通过。

[数组构造语法边界](#数组构造语法边界)的文档验收为：基本构造、两种嵌套写法、空数组定型、混合类型、非矩形与未闭合括号分别有树形、语义交接和诊断责任；PG/MySQL 正反例、参数/词法保护及证据限制齐全。文档完成不表示数组可解析、可绑定或可编码，所有运行行为未验证。

[行构造语法边界](#行构造语法边界)的文档验收为：ROW(1,2)、(1,2)、(1)、ROW(1, 四个核心输入分别区分语法合法、类型可判定与可执行；单字段 ROW 对照、顺序/嵌套、参数、关系括号及词法保护均有双方言责任和证据限制。当前全部运行行为未验证，不以解析或名称扫描成功替代语义兼容。

[引号与大小写规则](#引号与大小写规则)的文档验收为：foo、Foo、"Foo"、"a""b"、未闭合/空引号及字符串/注释均有双方言正反例、前提和诊断责任；词法键与后端存储、原始与转换位置区分明确。现有适配和测试不能证明完整名称解析，运行能力全部未验证。

[限定名称解析规则](#限定名称解析规则)的文档验收为：t、public.t、db.public.t、other.public.t 和 a.id 均区分当前改写/拒绝与 PG/MySQL 目标；别名隐藏、缺列、残缺/多段名有诊断责任及缺证说明；隔离检查不被文本降级绕过。所有运行行为仍未验证，测试阅读不构成通过。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)的文档验收为：基本 WITH、嵌套同名、public.t、定义顺序/前向、自引用及递归结构反例均有双方言目标；作用域生命周期、当前 RECURSIVE 拒绝和未验证项明确。当前测试仅为静态证据，所有解析、绑定、结果及诊断行为均未实测。

[模式搜索路径解析契约](#模式搜索路径解析契约)的文档验收为：public 在/不在路径、显式 public.t、catalog 同名及顺序、空路径、两 PG 会话和 MySQL 旁路均有目标及诊断责任；原始/有效路径、当前布尔门禁与完整目标区分清楚。全部运行行为仍未验证，状态/目录阅读不升级为绑定或兼容通过。

### 后续细分

[章节验收与后续交接](#章节验收与后续交接)将本章与第 1 章证据契约交工作流 3；实现先核实连接身份和最小原型，再按单一语法或名称规则重排依赖，逐项补运行证据。

[解析承载方案决策](#解析承载方案决策)后续先单独拆两候选最小原型取证和决策复审，再按语法保真、共享准备/规划接线、原 SQL 诊断、参数身份及双方言隔离分别细分；每项限定输入与消费者边界。共同类型/执行/目录缺口另行交接，禁止借原型一次补齐全部 PG parser。

[方言身份传递](#方言身份传递)后续先做连接身份拥有权及关闭/重连边界的局部取证，再分别拆简单查询与扩展查询的身份承接、共享解析/名称绑定/规划/表达式上下文的必要局部接线及隔离回归；实施前确认实际落点，不将本责任链自动扩张为完整子系统重建。各任务明确单一交接点、所需状态、现有可复用实现和验收断言；DIALECT-IDENTITY 三类场景是待新增验收入口，不是既有测试或已登记通过的能力。

[类型转换语法边界](#类型转换语法边界)后续先拆最小 `::` 解析承载取证，比较现有 Cast 与受控入口能否保留上述字段；再按优先级/嵌套、参数位置与词法保护、原 SQL 错误定位分别细分。每项复用现有 parser 与独立测试布局，只补必要局部接线；将转换语义及参数类型推断交工作流 3，不在此建立完整类型系统。

[大小写匹配语法边界](#大小写匹配语法边界)后续先核对 Rust parser 到现有载体的最小接线及 MySQL 扩展基线，比较共享 AST 与受控 PG 入口的保真成本；再分别拆谓词/NOT/比较组合、ESCAPE 信息与词法保护、原 SQL 诊断及隔离验收。沿现有独立测试布局补专项树形/位置断言，不递归修建执行子系统；匹配算法和转义求值另交工作流 3、4。

[返回子句语法边界](#返回子句语法边界)后续先按三种 DML 核对 Rust 解析动作到共享载体的最小保真，再拆表达式/星号/别名与原 SQL 位置、planner/Describe 列交接、双方言拒绝隔离。功能开放须等待工作流 5 的真实副作用/回滚和工作流 7 的结果描述证据，不递归补建执行或协议子系统。

[冲突子句语法边界](#冲突子句语法边界)后续先比较共享 AST 与受控 PG 入口保真，再分别拆目标/动作与双谓词、excluded 绑定、原 SQL 跨度与参数映射、双方言隔离的独立验收。先补解析树和拒绝证据；真实唯一性、并发及更新条件效果由工作流 5 单独细分，不递归扩建执行子系统。

[数组构造语法边界](#数组构造语法边界)后续先比较共享 AST 与受控入口保留构造类别、嵌套及跨度的最小方案，再拆基本/嵌套/空列表树形、原 SQL 定位、参数身份与双方言隔离验收；共同类型、形状语义及数组 OID 交工作流 3，Bind/Describe/结果编码交工作流 7 单独细分，不递归补建数组子系统。

[行构造语法边界](#行构造语法边界)后续先比较共享 Row 载体与受控 PG 入口保留显式性、嵌套和原 SQL 跨度的最小接线，再分别拆行/分组/子查询树形、名称扫描交互、错误位置及双方言隔离断言。沿独立测试布局取证，不递归扩建表达式或关系子系统；行类型与 NULL/比较按工作流 3、4 单独细分。

[引号与大小写规则](#引号与大小写规则)后续先取证共享名称载体的 quoted/拼写/跨度保真，再分别拆折叠与精确绑定、双引号转义及词法保护、位置映射和双方言隔离；后端同名异大小写的可区分性单独评估，禁止递归补建存储或目录子系统。

[限定名称解析规则](#限定名称解析规则)后续先取证关系分段与列限定的上下文保真及当前数据库身份，再分别拆关系限定/跨库拒绝、列与别名作用域、原 SQL 位置和双方言隔离断言；沿现有独立测试补 adapt/guard 的必要覆盖，不扩建目录或权限子系统。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)后续先核对顺序/嵌套/名称身份与跨度的保真，再分别拆普通 CTE 绑定与显式表绕过、前向及重复名诊断、查询块退出/跨语句与双方言隔离断言；递归入口依赖图与门禁另拆，不借名称任务补齐递归执行或物化子系统。

[模式搜索路径解析契约](#模式搜索路径解析契约)后续先核对状态到 resolver 的有效路径交接，再拆 public/显式限定、catalog 同名顺序与空路径、两会话及双方言隔离的独立验收；关系绑定时点与缓存失效先做参考端取证。任意 schema、权限、临时项及用户项分别交工作流 5、6 细分，不递归补建目录或会话子系统。

## 3. 共享类型与转换契约

### 模块

章节验收与后续交接以本章前18块为累计契约，依赖交付为 `f4cde1ed76`（准备描述执行元数据契约）。本轮复核 `pkg/server/pg_types_test.rs::common_type_oids_and_values`、`pkg/server/pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 以及 `pkg/server/pg_client_integration_test.rs::run_clients`：前两者分别有普通结果OID/值和部分参数成功/拒绝断言，后者启动真实客户端并检查MySQL隔离，不能外推全部类型差分覆盖。经RustCodeGraph读取，均未运行；pkg/server无doc.go。生产入口及各族独立测试沿前序模块段，不新增接口或测试文件。

[规范类型与协议映射](#规范类型与协议映射)复用共享内核；`pkg/types/field_type.rs::FieldType` 是 parser 类型别名，`AggFieldType` 保留 MySQL 混合符号整数提升规则，不以 PG 目标改写 Go 既有语义。`pkg/server/conn.rs::NativeType` 的 code/flags/length/decimal、`QueryResult` 与 `PreparedMetadata` 承接原始类型；`runtime.rs` 的 Prepare 分支及 `result_metadata` 从结果字段提取这些信息，`protocol_result_column` 另产 MySQL 列描述。PG 出口为 `pg_result.rs::pg_type` / `encode_formats` / `value_text` / `value_binary`；入口为 `pg_extended.rs::parameter` / `binary_parameter`。这些是现有符号，不是新建接口。第 2 章交来的转换目标、参数编号、数组结构及名称身份须先定型；本块不替 parser 或 resolver 决定语法与绑定。

本块静态取证基线为 `69ef39f91a781a4147568622943d719c95e3f7cd`，日期 2026-10-23。已阅读上述符号及独立测试 `pkg/server/pg_types_test.rs` 的 `common_type_oids_and_values`、`native_metadata_bridge_preserves_boolean_and_mysql_columns`、`unsigned_binary_and_unsupported_types`、`missing_original_metadata_is_explicitly_unsupported`、`catalog_char_and_acl_wire_types`、`pg_binary_native_results`，以及 `pkg/types/field_type_test.rs::TestFieldType` 的宽度、精度和 flags 断言。前者覆盖意图包括同值异类型、unsigned 最大值、bytea、JSON/零日期/duration 拒绝及缺元数据；目录 char/ACL 断言不能证明通用数组支持。目标包没有 `doc.go`。全部测试未运行，源码阅读不是当前功能通过证据。

[布尔类型契约](#布尔类型契约)沿用上述桥接，补充取证 `pkg/types/datum.rs` 的 Kind 标签和 `Datum::ToBool`（MySQL 风格真值转换，不能直接作为 PG 隐式转换），以及 `pg_extended.rs::parameter` / `binary_parameter` / `Extended::process`、`pg_result.rs::pg_type` / `value_text` / `value_binary`。静态基线为 `9b3d77f784`（2026-10-23），目标包无 `doc.go`；独立测试 `pg_types_test.rs::common_type_oids_and_values` 断言 TRUE/FALSE 的 OID 16 与 t/f，`native_metadata_bridge_preserves_boolean_and_mysql_columns` 断言同值整数与布尔的 flags 区别，`pg_binary_native_results` 含 false→00；`pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 只含 true 成功和 unknown 拒绝的布尔参数断言；`pkg/types/datum_test.rs::test_to_bool` 是共享真值转换意图，不证明 PG 转换规则。以上测试均未运行。

[整数与无符号边界](#整数与无符号边界)取证基线为 `01ce26d145`。实际读取 `pkg/server/pg_result.rs::pg_type/value_text/value_binary`、`pkg/server/pg_extended.rs::parameter/binary_parameter/Extended::process`、`pkg/types/datum.rs::KindInt64/KindUint64/Datum` 与 `pkg/types/convert.rs::IntegerUnsignedUpperBound/IntegerSignedUpperBound/IntegerSignedLowerBound/ConvertIntToInt/ConvertUintToInt/ConvertIntToUint`。两个包均无 doc.go。独立测试已读 `pkg/server/pg_types_test.rs::unsigned_binary_and_unsupported_types/pg_binary_native_results/common_type_oids_and_values`、`pkg/server/pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 和 `pkg/types/convert_test.rs::test_convert`；它们是断言意图，均未运行。Datum 区分 i64/u64 承载，但不能单靠值盒确定 int2/int4/int8 声明身份。

[精确数值契约](#精确数值契约)取证基线为 `7fe5fefab8`（本轮只改文档）：[mydecimal.rs](../pkg/types/mydecimal.rs) 的 MyDecimal、fit_unscaled、FromString、Round、ToBin/FromBin，与独立 [mydecimal_test.rs](../pkg/types/mydecimal_test.rs) 的 TestRoundWithHalfEven、TestRoundWithTruncate、TestToBinFromBin；HalfEven 名称实际调用 ModeHalfUp，不能按名字推定银行家舍入。MyDecimal 以 9 个 base-10^9 word 分别容纳整数和小数，容量按两部分各自向上取整占 word，不能简单承诺任意 81 位组合；BigInt 是运算中间量，不表示存储域无限。MAX_DECIMAL_SCALE=30 用于特定编码/运算限制，不是所有方法统一的 PG scale 上限。

cast 另见 [builtin_cast.rs](../pkg/expression/builtin_cast.rs) 的 to_decimal、parse_decimal、produce_decimal：使用 rust_decimal::Decimal，解析失败可经浮点回退，负 scale 被 max(0)，超域产生 warning 并裁剪；常量 MAX_DECIMAL_WIDTH=65 不能证明实际能精确承载 65 位。参数见 [pg_extended.rs](../pkg/server/pg_extended.rs) 的 parameter/binary_parameter；结果见 [pg_result.rs](../pkg/server/pg_result.rs) 的 pg_type、value_text、value_binary、encode_formats。独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 text_parameters_are_typed_and_reject_loss 仅含 12.340 接受及 12bad 拒绝意图；[pg_types_test.rs](../pkg/server/pg_types_test.rs) 有 DECIMAL(8,2)→OID 1700/text 12.34 断言及 U64 文本入口。均未运行，不证明 PG 大精度、typmod、特殊值或 binary 支持；上述三个包均无 doc.go。

[浮点数值契约](#浮点数值契约)静态取证基线为 `cae627a7dc`（2026-10-23，批次 4 精确数值契约已交付）。`pkg/types/datum.rs::Datum` 的 GetFloat32/SetFloat32、GetFloat64/SetFloat64 分别保留 KindFloat32/KindFloat64；SetFloat32FromF64 可将未窄化 f64 写入 float32 标签，标签本身不能证明值已符合 float4 域。`pkg/types/convert.rs::format_float` 使用 ryu 最短格式并展开指数，不是 PG 输出格式的证据。PG 边界仍为 `pg_extended.rs::parameter/binary_parameter` 和 `pg_result.rs::pg_type/value_text/value_binary/encode_formats`：700/701 参数分别解析 f32/f64，再转共享参数小端编码；binary 输入从网络序转小端。结果 Value::Float 共用 f64，text 按 f64 输出，binary 先经文本再解析到目标宽度，尚无完整窄化溢出和 float4 文本定宽证据。

已读独立测试 `pkg/server/pg_types_test.rs::pg_binary_native_results`，浮点部分含 float4 1.5、float8 正无穷及两者 SQL NULL 长度 -1 的 binary 输出断言；`common_type_oids_and_values` 没有浮点场景。另读 `pkg/server/pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 与 `pkg/types/convert_test.rs::test_convert_to_string`，后者有 1.6_f32/-0.6_f64 的共享输出意图，不能替代 PG 可逆文本验收。两个目标包无 doc.go；所有测试均未运行，未见上述断言覆盖完整浮点边界，不把静态路径或 OID 视为功能通过。

[文本类型契约](#文本类型契约)静态基线为 `e3c4a89ce7`（批次 5 已交付）。已读 [field_type.rs](../pkg/types/field_type.rs) 的 InferParamTypeFromDatum/InferParamTypeFromUnderlyingValue：字符串推断补 MySQL charset/collation，可变长度设 UnspecifiedLength；[datum.rs](../pkg/types/datum.rs) 的 SetString/GetValue 保留 KindString 和 collation，SetBytes 另设 KindBytes。这些信息不能单独区分 PG text/varchar/bpchar 身份或声明长度。目标包 pkg/types、pkg/server 均无 doc.go。

PG 入口 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter 先校验 UTF-8/NUL，再将 25/1043/1042 同转 tp=253；binary_parameter 对三者复用 parameter。NULL 提前返回 tp=6，typed NULL 连续性缺证。出口 [pg_result.rs](../pkg/server/pg_result.rs)::pg_type 将普通非 binary 字符串映射为 25，charset=63 则为 bytea；value_text 拒绝 NUL，value_binary 的字符串分支发送文本字节，encode_formats 固定 typmod=-1。不能从此宣称已保留 varchar/char 长度、补空格或跨格式身份。

已读独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 的 hello→25/NULL 断言、catalog_char_and_acl_wire_types 的受控目录 1042 断言，以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 的 VARCHAR(30) 表入口。目录特例不证明普通 char 支持，上述片段不覆盖本块完整边界；所有测试未运行。

[字节类型契约](#字节类型契约)静态基线为 `c559d9ad7b`（2026-10-23，批次 6 已提交）。通过 RustCodeGraph 读取 [binary_literal.rs](../pkg/types/binary_literal.rs)::BinaryLiteral/String/ToString/ToInt/Compare：Vec<u8> 能保存字节，但 ToString 使用 from_utf8_lossy，ToInt/Compare 去前导零，不能直接作为 bytea 保真转换或比较。独立 [binary_literal_test.rs](../pkg/types/binary_literal_test.rs)::test_binary_literal/test_parse_hex_str_table 是 MySQL BIT/HEX 意图（如 0x1 补零），不能证明 PG 输入语法。pkg/types、pkg/server 均无 doc.go。

现状取证 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter：OID 17 text 先验证 UTF-8/NUL，再要求 `\x` 前缀及连续偶数 hex 数字；非 hex 返回 0A000，奇数长度/非法数字返回 22P02，不接受 PG 合法的字节对间空白或 escape 输入。解码后与 binary 原字节入口都转 tp=252；NULL 提前转 tp=6，身份连续性缺证。[pg_result.rs](../pkg/server/pg_result.rs)::pg_type/value_text/value_binary/bytea/encode_formats 只对 string/blob 家族且 ColumnInfo.charset=63 选 OID 17，NativeType 本身没有 charset；结果 text 固定小写 hex，binary 的 Value::Bytes 直接复制。Value::Text 回退不能证明任意 bytes 保真。

已读独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::unsigned_binary_and_unsupported_types 的 00 ff 5c→17/文本 `\x00ff5c`、pg_binary_native_results 的 00 ff 原字节结果，以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss 的 `\x00ff` 成功/`\xabc` is_err。测试全部未运行，不证明完整往返、全字节域、精确错误或 typed NULL 元数据。

[日期类型契约](#日期类型契约)取证基线为 `aa462efc89`（批次 7 已提交）。按仓库 RustCodeGraph 读取 [time.rs](../pkg/types/time.rs)::ParseDate/ParseTimeFromNum：复用 MySQL TypeDate、fsp=0，整数零可构造零日期；[core_time.rs](../pkg/types/core_time.rs)::CoreTime/getYear/setYear 为 u64 压缩字段，年份提取为 u16，写入受掩码约束，不是 PG 有符号日序或扩展年份的保真证据。两个目标包均无 doc.go。

入口 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/temporal 对 1082 用 chrono `%Y-%m-%d`，限制年份 1..9999，再转 MySQL tp=10 的年月日小端参数；无效日期当前统一 22007，超引擎年份为 0A000；binary_parameter 没有 1082 分支，非 NULL 返回 0A000，NULL 提前为 tp=6。[pg_result.rs](../pkg/server/pg_result.rs)::pg_type 将 10/14 映射 1082/4；value_text 仅粗查年非零、月 1..12、日 1..31，不能排除 2023-02-29 或 04-31；value_binary 再用 chrono 验证并输出相对 2000-01-01 的 i32 网络序日数，存在两格式校验差异。encode_formats/Extended::process 的 Describe 一致性检查不能替代日期身份、typed NULL 和错误分类取证。

独立 [time_test.rs](../pkg/types/time_test.rs) 的 ParseDate 实际断言（约 374–400 行）含短年份、特殊分隔符和 2011-02-30 拒绝，Go fixture 字符串本身不算执行断言；[pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss 含 2024-02-29 接受、2026-02-29 拒绝但未断言 SQLSTATE；[pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 有 2026-09-30→1082/text，pg_binary_native_results 有 1999-12-31→-1 日及逐类型 NULL 输出断言。以上全部未运行，不能证明 PG 全域、两方向往返或准备元数据连续性。

[日内时间类型契约](#日内时间类型契约)静态基线为 `a8a704187c`（批次 8 日期块已提交）。通过 RustCodeGraph 读取 [time.rs](../pkg/types/time.rs)::ParseDuration/matchDuration/Duration::RoundFrac、MinTime/MaxTime/MaxFsp，以及 [datum.rs](../pkg/types/datum.rs)::KindMysqlDuration/KindMysqlTime：共享 TIME 是可负、可超过一天的纳秒 duration，Fsp 与日内身份不可混同。Duration 的日期转换可依赖当前日期和 location，不能直接当 PG time cast。pkg/types、pkg/server 均无 doc.go。

入口 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/temporal 以 chrono `%H:%M:%S%.f` 解析 1083，非整微秒返回 0A000，再转 MySQL tp=11 的非负、零天数结构；binary_parameter 无非 NULL 1083 分支，返回 0A000；NULL 提前转 tp=6。出口 [pg_result.rs](../pkg/server/pg_result.rs)::pg_type/value_text/value_binary/encode_formats 将 code=11 映射 1083/8，typmod 固定 -1；text 只检查小时 <24，binary 另用 chrono 校验分秒与微秒。该差异不能证明两个格式同域，且当前拒绝合法 PG 24:00 边界。

已读独立 [time_test.rs](../pkg/types/time_test.rs)::TestToNumber 的实际 ParseDuration 断言（含负值及小数舍入；go_cases 字符串不作为执行断言）；[pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 的 12:34:56→1083/text、unsigned_binary_and_unsupported_types 的 25:00:00 拒绝、pg_binary_native_results 的 1 微秒→i64 网络序 1 与该类型 NULL 输出；[pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss 明确断言 24:00:00 参数 is_err，不能沿用为 PG 目标。全部测试未运行，不证明完整往返、精度或准备元数据连续性。

[无时区时间戳契约](#无时区时间戳契约)静态取证基线为 `c8f463e422`（2026-10-23，批次 9 已提交）。经 RustCodeGraph 读取 [time.rs](../pkg/types/time.rs) 的 TimeContext、CoreTime 位域、Time::ConvertTimeZone/String：共享值是 MySQL 日历字段、类型标签与 Fsp；ConvertTimeZone 显式按 from/to 转换字段，不能证明 PG 无时区身份。Go 对照 [time_test.go](../pkg/types/time_test.go)::TestConvertTimeZone 有 UTC/Asia/Shanghai 相差八小时及零值保留断言；这不是 PG timestamp 的规则或本轮实测。两个目标包均无 doc.go。

入口 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/temporal/binary_parameter/Extended::process：1114 text 经 chrono 固定 `%Y-%m-%d %H:%M:%S%.f`，限年 1–9999、整微秒，转 MySQL tp=12；binary 非 NULL 1114 未开放（0A000），NULL 提前转 tp=6。出口 [pg_result.rs](../pkg/server/pg_result.rs)::pg_type/value_text/value_binary/encode_formats 将 code=7/12 都映射 1114/8，typmod 固定 -1；text 仅粗查年月日，binary 经 chrono 解析后取相对 2000-01-01 的微秒。MySQL TIMESTAMP 上游会话转换是否已执行须另取端到端证据，不从该映射推断等价。

已读独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 的 DATETIME→1114/text、pg_binary_native_results 的 1999-12-31 23:59:59.999999→网络序 i64 -1 及 NULL 长度 -1；[pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss 含六位小数接受、七位拒绝，后者不能作为 PG 舍入目标。所有测试未运行，不证明两格式往返、特殊值、全范围、会话隔离或 Describe 连续性。

[带时区时间戳契约](#带时区时间戳契约)静态基线为 `ba1af304d2`（2026-10-23，批次 10 无时区时间戳已提交）。经 RustCodeGraph 核对 [time.rs](../pkg/types/time.rs)::TimeContext/CoreTime/ConvertTimeZone：位域承载日历字段和 Fsp，转换要求显式 from/to 并重写字段；不能单独保留 PG 瞬时身份、全域或非有限值。[pg_session.rs](../pkg/server/pg_session.rs)::PgSession 当前字段为 path/failed_transaction，不能据此证明 PG TimeZone 状态已接通。两个包无 doc.go。

已读 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter/temporal 和 [pg_result.rs](../pkg/server/pg_result.rs)::pg_type/value_text/value_binary：非 NULL OID 1184 参数两格式均无分支，返回 0A000；NULL 提前返回 tp=6，不证明 typed NULL 保真；结果 code=7/12 均映射 1114，没有独立 1184 身份。独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss 仅断言 1114 六位成功、七位拒绝；[pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values/pg_binary_native_results 的 DATETIME→1114、-1 微秒及 NULL 不证明 timestamptz。Go [time_test.go](../pkg/types/time_test.go)::TestConvertTimeZone 断言 UTC/Shanghai 八小时转换及零值保留，仅为原 MySQL 行为对照。以上测试均未运行。

[时间间隔契约](#时间间隔契约)静态基线为 `7ba687df06`（批次 11 已提交）。经 RustCodeGraph 核对 [time.rs](../pkg/types/time.rs)::Duration/ParseDurationValue/ExtractDurationValue：Duration 只存 i64 纳秒总量与 Fsp；ParseDurationValue 返回年/月/日/纳秒分量，但 ExtractDurationValue 将月按 30 天折入总量并应用 MySQL TIME 范围，不能保真承接 PG interval。[datum.rs](../pkg/types/datum.rs)::KindMysqlDuration 是 MySQL TIME 标签，没有由该标签证明的 PG interval 身份。[builtin_time.rs](../pkg/expression/builtin_time.rs) 使用 chrono Duration 与 MySQL TIME 微秒上限，也不证明日历间隔身份。目标三包均无 doc.go。

[pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter 没有非 NULL OID 1186 分支，当前返回 0A000；NULL 提前转 tp=6 不证明 typed NULL 保真。[pg_result.rs](../pkg/server/pg_result.rs)::pg_type 将 code=11 映射为 1083/8，没有通用 interval 映射；不能把 MySQL TIME 的结果 OID 改成 1186 就声称支持。[time.go](../pkg/types/time.go)::ExtractDurationValue 也有 month*30 折算，这是 MySQL 对照，不是 PG 目标；本任务不修复或简化原 Go/Rust 行为。

已读独立 [time_11_aster_unit_test.rs](../pkg/types/time_11_aster_unit_test.rs) 的实际断言：ParseDurationValue 保留 DAY_MICROSECOND 的 day=12，ExtractDurationValue(MONTH,1) 输出 720:00:00，DAY=-35 拒绝；duration_parse_arithmetic_and_format_match_go 将负两天三小时折为 -51 小时。[pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 和 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 提供其他类型与协议入口，不能证明 1186 支持。所有测试均未运行，本块专项用例均待新增。

[通用唯一标识契约](#通用唯一标识契约)静态基线为 `fda2b60876`（批次 12 已提交，2026-10-23）。经 RustCodeGraph 阅读 [datum.rs](../pkg/types/datum.rs)::Datum/Kind 常量，没有独立 UUID 标签；KindString/KindBytes 只证明通用载体。[field_type.rs](../pkg/types/field_type.rs)::InferParamTypeFromDatum/InferParamTypeFromUnderlyingValue/DefaultTypeForValue 按 MySQL 类型推导，String 为 TypeVarString、NULL 为 TypeNull，不从字符串外观保留 UUID 身份。目标包 pkg/types、pkg/server 均无 doc.go。

[pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter 没有非 NULL OID 2950 分支，分别走 0A000；NULL 提前返回 tp=6，不能证明 typed NULL 保真。[pg_result.rs](../pkg/server/pg_result.rs)::pg_type/value_text/value_binary 没有通用 UUID 映射及 binary 编码；字符串/二进制列现有映射为 text/bytea，不构成 UUID 支持。已读独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss（整数、bool、bytea、时间、decimal 与 NULL）及 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values/unsigned_binary_and_unsupported_types（常见类型、bytea 与拒绝路径），这些断言未覆盖 UUID，且本轮均未执行。不改变原 Go/MySQL 字符串及 bytes 行为。

[JSON文本契约](#JSON文本契约)静态基线为 `c6807d8e195f20a13b3eb32178fc85c39dbc09b8`（批次13 UUID已提交，2026-10-23）。经仓库 RustCodeGraph 阅读 [json_binary.rs](../pkg/types/json_binary.rs)::ParseBinaryJSONFromString/UnmarshalJSON/from_serde_value/appendBinaryNumber/MarshalJSON/marshalObjTo：输入经 serde 值树与 BTreeMap，再编码 i64/u64/f64；不保留原始空白、重复成员及数字词法，对象按键重建。Number(String) 中间态不证明最终任意精度，超过整数域会走 f64。输出重新插入分隔空格并转义字符串，不是原文出口。最大深度100是现有MySQL限制，不能直接宣称为PG限制。

独立 [json_binary_test.rs](../pkg/types/json_binary_test.rs)::TestBinaryJSONMarshalUnmarshal 仅断言规范文本往返，TestMarshalFloatInContainer 断言浮点格式化，TestParseBinaryFromString 检查空文档/尾随内容；未见这些用例断言 PG 原文、重复键或超numeric大数保留。[builtin_json.rs](../pkg/expression/builtin_json.rs)::json_null/argument_value/json_unquote 表明函数特定的SQL NULL→JSON null及解引号逻辑，不是通用cast依据。[pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter 无非NULL OID114分支，走0A000；[pg_result.rs](../pkg/server/pg_result.rs)::pg_type 无code245映射。[pg_types_test.rs](../pkg/server/pg_types_test.rs)::unsigned_binary_and_unsupported_types 断言JSON及NULL JSON结果拒绝。三个目标包无doc.go；所有测试均未运行，输入与输出证据均为静态阅读。

[JSONB规范值契约](#JSONB规范值契约)静态基线为 `49bc0d486d`（批次14 JSON文本块已提交，2026-10-23）。经 RustCodeGraph 核对 [json_binary.rs](../pkg/types/json_binary.rs)::ParseBinaryJSONFromString/UnmarshalJSON/appendBinaryNumber/appendBinaryObject/from_serde_value：MySQL BinaryJSON 使用 i64/u64/f64、BTreeMap 和自有类型码/长度表，支持 Opaque/Time/Duration 扩展；它不是 PG jsonb 规范值或 wire。数字字符串最终可能转 f64，对象字典序也不能证明 PG 输出键序一致，深度100/键长限制不能直接作为PG目标限制。沿用原 Go/MySQL 语义，不为 PG 改写这些函数。

独立 [json_binary_test.rs](../pkg/types/json_binary_test.rs)::TestBinaryJSONMarshalUnmarshal 已读断言嵌套、数组、null、浮点的 MySQL 规范文本往返，不证明重复键最后值、任意精度或PG binary版本兼容。[builtin_json.rs](../pkg/expression/builtin_json.rs)::json_null/argument_value 是函数特定 NULL 转换，不是通用 jsonb cast。[pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter 无非NULL OID3802分支；[pg_result.rs](../pkg/server/pg_result.rs)::pg_type 未提供code245映射，独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::unsigned_binary_and_unsupported_types 明确拒绝JSON及NULL JSON结果。三个目标包无doc.go；本轮仅静态阅读，未运行测试，以上均不能标作PG jsonb已支持。

[基础数组契约](#基础数组契约)静态基线为 `021e3ceeab`（批次15已提交，2026-10-23）。经 RustCodeGraph 阅读 [datum.rs](../pkg/types/datum.rs)::Datum 及 Kind 常量：现有标量盒、KindInterface、MySQL JSON 和 float32 vector 不提供通用 PG 数组的元素身份、形状与下界契约，不能以任意载荷存在推导支持。[pg_result.rs](../pkg/server/pg_result.rs)::CatalogColumnType/wire_type/pg_type/value_text/value_binary/binary_array/encode_formats 有目录专用数组 OID；binary_array 从目录文本生成一维载荷，普通数组下界固定1，vector下界0，拒绝嵌套，不能承接任意下界或多维 SQL 值。value_text 对文本直接输出，不构成通用数组校验器。

[pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::process 的普通 Parse 白名单不含数组OID；parameter/binary_parameter 没有非NULL通用数组解码，NULL参数先转code6也不证明typed NULL身份保真。已读独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::pg_binary_native_results 的目录空int2数组（ndim=0、元素OID21）及int2vector（下界0）字节断言、catalog_char_and_acl_wire_types 的ACL NULL/OID断言；后者不是ACL非NULL二进制或普通数组支持证据。[pg_oid_test.rs](../pkg/server/pg_oid_test.rs)::pg_introspection_oid_ranges_and_local_indexes 只断言目录对象OID区间/唯一性，未覆盖元素/数组OID或SQL数组往返。目标包无doc.go；这些测试均未运行，本轮仅静态取证。

[显式隐式转换契约](#显式隐式转换契约)静态基线为 `1ecb906fd0`（批次16基础数组已交付，2026-10-23）。经 RustCodeGraph 阅读 [builtin_cast.rs](../pkg/expression/builtin_cast.rs)::BuildCastFunctionWithCheck/cast_value：目标元数据在构建时固定，输入 NULL 在求值时直接返回 NULL；数组检查为 MySQL JSON 到 typed array 的局部路径，不是 PG 数组转换。其 FieldKind、CastContext 和 in_union 不能代替 PG 类型身份及三种转换上下文。[datum.rs](../pkg/types/datum.rs)::ConvertTo 按 MySQL Type 分派且 NULL 直接返回默认 Datum；[convert.rs](../pkg/types/convert.rs)::handled_truncate/ConvertIntToInt/ConvertFloatToInt 使用 Context 截断策略及带裁剪值的错误，不能直接作 PG 错误后仍返回结果的依据。[pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::process 在普通 Parse 将未指定 OID 补0后明确以0A000拒绝；Bind依据固定OID选择parameter/binary_parameter，格式码不参与SQL转换资格。

已读独立 [builtin_cast_test.rs](../pkg/expression/builtin_cast_test.rs) 的挂载入口及 [builtin_cast_4_aster_unit_test.rs](../pkg/expression/builtin_cast_4_aster_unit_test.rs)::cast_numeric_matrix_matches_go_rounding_and_unsigned_rules、decimal_cast_rounds_and_clamps_to_declared_precision、json_conversion_preserves_parse_flag_and_typed_array_semantics：分别断言负数转unsigned告警、科学计数法前缀截断、decimal溢出钳制和JSON数组转换。[convert_test.rs](../pkg/types/convert_test.rs)::test_str_to_num 明确断言忽略截断时“11xx”→11、告警模式空串→0；这些是需保留的Go/MySQL意图，不是PG证据。[pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 已读目录专用Parse/Bind/Describe局部断言，不能据此推导通用参数推断或转换矩阵覆盖。pkg/expression、pkg/types、pkg/server均无doc.go；所有测试只读未运行，PG转换资格、共同类型和SQLSTATE均未验证。

[准备描述执行元数据契约](#准备描述执行元数据契约)静态基线为 `d42b55c369`（批次17已提交）。通过 RustCodeGraph 已读 [conn.rs](../pkg/server/conn.rs)::PreparedMetadata/QueryResult、[pg_extended.rs](../pkg/server/pg_extended.rs)::Statement/Portal/Extended::process/description/description_formats，以及 [pg_result.rs](../pkg/server/pg_result.rs)::encode_formats；pkg/server/doc.go 不存在。PreparedMetadata 只有参数数量而无推定参数类型；PG Statement 另存 oids/mapping。普通 Parse 对 OID 0 或未提供的类型返回0A000；目录专用推断不能证明通用参数表达式已支持。Bind 保存独立列、native_types、description及formats；Describe 不调用执行闭包；Execute 编码后比对准备描述，不一致返回0A000，并移除内部生成的RowDescription后发结果行。无参数且执行缺native_types时有按列数回填准备元数据的限定分支，不代表参数化投影可同样回填。encode_formats核对类型数量及行宽，但统一typmod=-1、来源表/属性为0，仍缺完整规范身份及修饰符保真。

已读独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 中目录零行两种Describe、显式23参数、多值/NULL重绑定、重复乱序参数和缺OID拒绝断言；[pg_types_test.rs](../pkg/server/pg_types_test.rs)::missing_original_metadata_is_explicitly_unsupported、unsigned_binary_and_unsupported_types、pg_binary_native_results 分别提供缺原类型、数量冲突及局部binary/NULL断言意图。测试均未运行；现有片段未证明通用参数化表达式、全部typmod、DDL后类型变化或无查询探测的全链验收。

### 目标行为

#### 规范类型与协议映射

本块建立方向契约，不把 MySQL 类型名、PG OID 和规范引擎类型视为同一身份。这里“规范类型”指执行与准备阶段保留的类型身份、修饰符和值域约束，并不声称现有 `NativeType` 已完整表达所有 PG 类型。值相同仍可属于 bool、int2、int4、int8 或 numeric；类型描述必须先于行内容确定。双向映射允许提升或拒绝，不承诺可逆。

方向与共同约束如下；下表 NULL、错误和元数据规则适用于后续每个类型族。

| 方向 | 类型身份、值域与精度 | NULL 与错误 | wire 元数据、现状与责任 |
| --- | --- | --- | --- |
| PG 参数 → 规范值 | 先按 Parse 声明 OID、SQL 目标和上下文推断确定参数身份，再按 Bind 格式解码；OID 0 表示尚未指定，不能当 text 或零值。保留参数序号与重复引用的同一身份，检查目标值域/精度。 | 参数长度 -1 是 SQL NULL，不解码值但仍校验类型；空载荷不是 NULL。类型无法确定须拒绝，非法文本/二进制与溢出分开。 | ParameterDescription 返回确定的 OID，Bind 格式 0/1 只决定编码，不决定类型；没有参数 typmod 字段，SQL 目标约束仍须保留。现有 parameter 将若干 OID 转成 BinaryParam，NULL 仅留下 tp=6；binary_parameter 支持集合更小。类型保真由本章主责，消息与恢复交工作流 7。 |
| MySQL 参数 → 规范值 | 按自身参数类型及 unsigned 等标志解码；这些输入不是 PG OID。保留原 MySQL 域，不从 PG 反向猜测 unsigned 或 bool。 | SQL NULL 保留空值及独立类型信息；错误遵循 MySQL 原契约，不能套 PG SQLSTATE。 | conn/conn_stmt 的既有 MySQL 参数边界继续拥有 wire 解释；本块只约束共享桥接不得丢失信息，未验证全链参数保真。 |
| 解析/推导 → 规范类型 | 消费第 2 章保留的类型名、修饰符、数组结构及上下文；共同类型和 cast 决策先按方言确定，再复用 FieldType 能保真的部分。 | 有类型 NULL 沿用声明；无类型字面量/参数依 PG 上下文解析，不能一律映射 code=6 的 text 回退。缺身份拒绝，不读首行补全。 | Prepare 与执行输出应携带同一类型契约；现有 PreparedMetadata 只有结果 NativeType 与 parameter_count，未证明参数身份和所有 PG 修饰符保真。类型推导属本章，运算重载协作工作流 4。 |
| 规范类型/值 → PG 结果 | 先生成每列 OID、typlen、typmod，再编码每个值；同列不因数值大小、首行/后续行、NULL 或格式而改变类型。 | SQL NULL 的 DataRow 长度 -1、无载荷，仍有列类型；未知类型即使无行或全 NULL 也拒绝。值域不符不得截断、转 text 或改 OID 掩盖错误。 | pg_type 已按元数据选 OID；encode_formats 校验列/native 数量，当前 typmod 恒 -1，来源表/属性号为 0。本章给类型约束，工作流 7 负责 Describe/Execute 与错误传输及恢复。 |
| 规范类型/值 → MySQL 结果 | 保持 MySQL code、flags、charset、length、decimals 的原契约；PG 类型无保真对应时须有显式转换或拒绝。 | NULL 不改变列定义；不因 PG 映射扩大或缩减 MySQL 原域。 | ColumnInfo 与现有 MySQL 编码继续拥有 wire 描述；PG OID/typlen 不写入 MySQL code/length。PG 专属类型的 MySQL 暴露策略未取证，不能默认为字符串。 |

下表是**目标方向映射**，括号内为当前 PG 出口的局部事实；MySQL 一列为共享已有类型的承载约束，不宣称 PG 专属类型已有 MySQL 等价物。U 表示 UnsignedFlag，B 表示 IsBooleanFlag；字符集二进制身份须单独保留，不能用值是否可打印决定。每行 PG 三元组是 OID/typlen/typmod；typlen 是 PG 类型内部长度标识，既非实际 wire 载荷长度，也非 MySQL 显示宽度。无修饰符用 -1；有修饰符必须来自定型结果。

| 规范身份/保真要求 | MySQL wire 方向（输入/输出） | PG 方向（输入身份 → 结果三元组） | 损失、拒绝与当前缺口 |
| --- | --- | --- | --- |
| boolean，独立于整数 0/1 | 保留内核 B；既有 MySQL 列可与整数相同，不强造 wire bool 身份 | bool 16 → 16/1/-1 | 仅凭 TINYINT 或值 1 不能判 bool；现有 pg_type 优先 B，参数 bool 到 tp=1 后的身份连续性未证明。 |
| 有符号整数，精确位宽 | TINY=1、SHORT=2、LONG=3、INT24=9、LONGLONG=8，U 清除；不要把显示宽度当位数 | int2 21、int4 23、int8 20 → 分别 21/2/-1、23/4/-1、20/8/-1 | PG 域为有符号 16/32/64 位。现有 tiny→int2、short→int2、long/int24→int4、longlong→int8；PG 声明 int2 输入不能因值小变 tiny 身份。越界拒绝。 |
| 无符号整数，完整原域 | 同上 code + U，值不能先窄化到 i64 | 无 PG unsigned 输入类型；共享结果 U8/U16/U24/U32/U64 → int2/int4/int4/int8/numeric | 对应结果 21/2/-1、23/4/-1、23/4/-1、20/8/-1、1700/-1/-1；最大 u64 必须精确保留 18446744073709551615。当前有 tiny/short/long/longlong 提升分支；不从结果 numeric 反推出 unsigned，反向须有声明及范围校验。 |
| exact numeric，精度/有符号 scale | DECIMAL=0 或 NEWDECIMAL=246，length/decimal 只在语义相容时复用 | numeric 1700 → 1700/-1/定型 typmod；无约束为 -1 | 不经 f64；不能把 MySQL decimal 上限当 PG 上限。当前文本参数用 MyDecimal 校验，binary 参数未列入支持分支；结果 typmod 丢失。负 scale、特殊值与超出共享表示的域须独立取证，不能靠映射宣称覆盖。 |
| float4 / float8，各自精度 | FLOAT=4 / DOUBLE=5，保留位宽 | 700 → 700/4/-1；701 → 701/8/-1 | 不能以当前值可表示为由降精度；NaN、无穷、负零及溢出按族契约取证。现有参数/结果有分支，不代表特殊值行为已验收。 |
| text / varchar(n) / bpchar(n)，保留字符长度与身份 | VARCHAR=15、VAR_STRING=253、STRING=254 及文本 blob 家族，沿自身 charset/flags；PG char 不能等同内部目录 char | 25 → 25/-1/-1；1043、1042 → 各自 OID/-1/(n+4)，无约束 -1 | PG 长度为字符约束，不取 MySQL wire 字节宽度直接代入。当前普通文本出口折为 text；入口 25/1043/1042 同转 tp=253，声明身份/补空格语义未证明。非法编码和不允许的 NUL 拒绝。 |
| bytea，原始字节 | 二进制 charset 与 blob/string 家族；不能与同载荷 text 合并 | 17 → 17/-1/-1 | 可含零字节；文本转义与 binary 载荷分开验收。现有参数文本只收 hex，binary 可收原字节；结果按 charset=63 选 bytea，此信息不在 NativeType 内，桥接仍依赖 ColumnInfo。 |
| date，日历日期 | DATE=10 / NEWDATE=14，不能用零日期冒充 PG 日期 | 1082 → 1082/4/-1 | 共享范围之外及 MySQL 零日期须明确拒绝；现有结果有映射，参数 temporal 限制引擎日期域，不证明 PG 完整日期域或 infinity。 |
| time，日内时间，秒小数精度 | TIME=11 的 duration 域须与日内时间区分，非无条件双向 | 1083 → 1083/8/p；未声明 p 为 -1 | p 为 0–6。负 duration、25:00:00 不能无损映射日内时间，拒绝或走显式转换；24:00 边界独立取证。当前 code 11 选 time，不证明域已相同。 |
| timestamp，无时区墙钟时间 | DATETIME=12 / TIMESTAMP=7 不能仅凭名称判为 PG 瞬间 | 1114 → 1114/8/p；未声明 -1 | 当前 7/12 都映射 1114；精度、零日期及范围须校验，不能添造时区。MySQL TIMESTAMP 的会话处理不能证明 PG timestamp 等价。 |
| timestamptz，瞬间身份 | 现有 MySQL code 无独立保真对应；须明确转换与会话前提，禁止自动借 7 | 1184 → 1184/8/p；未声明 -1 | 不要求保留原输入时区拼写，但必须保留瞬间，显示依会话时区；现有 NativeType 四字段及 pg_type 没有此身份分支。缺信息拒绝，不冒充 1114。 |
| interval，月/日/时间分量 | 无直接 MySQL duration 等价；须显式转换或拒绝 | 1186 → 1186/16/字段掩码及 p 的类型修饰符；无约束 -1 | 月不折固定秒数；不能仅靠 TIME code 保真。当前通用出口未见分支；字段掩码/typmod 精确规则交 interval 块取官方源码证据，开放受限类型前必须补齐。 |
| UUID，128 位身份 | 无专属既有 wire code；binary/text 仅可作显式转换的结果 | 2950 → 2950/16/-1 | 不从字符串形状或 16 字节反推 UUID；缺规范身份或非法输入拒绝。当前通用 pg_type 未见 UUID 分支。 |
| JSON / JSONB，彼此独立身份 | JSON=245 不能证明同时保留两者语义；文本转换必须显式 | 114 → 114/-1/-1；3802 → 3802/-1/-1 | JSON 原文保真与 JSONB 规范化不可互换，JSON null 不是 SQL NULL；现有 code 245 出口明确拒绝，NULL 也不能绕过。参数/结果 codec 与范围交各族文档，不能用 text OID 假成功。 |
| 基础数组，元素类型、维度、下界及元素 NULL | 无 MySQL 通用数组 wire 等价，不能把字符串花括号视为规范数组 | 由元素身份确定独立数组 OID（int4[] 示例 1007），typlen=-1；typmod 承接元素修饰，不代表维度 | 数组 NULL、空数组、含 NULL 元素分开；必须保留形状/下界，不从首元素猜类型。当前 CatalogColumnType 有目录专用数组/vector 与编码，不证明任意表达式/参数数组支持。其他族数组 OID 在各族开放前核对官方目录。 |

numeric typmod 目标按官方 `make_numeric_typmod` 的精度高 16 位、scale 低 11 位编码并加 4，scale 的符号扩展按 `numeric_typmod_scale`，不能直接套 MySQL decimal 字段。字符 typmod 的 n+4 来自 `anychar_typmodin`。时间族与 interval 的精确修饰符、特殊域及各族 binary 布局在相应文档块取证；本块不发明格式或将其标为现有实现。未知引擎 code、缺必要规范字段和仅有目录专用身份，目标为显式不支持路径；不静默降级 text，不通过读取首行“恢复”身份。

方案比较：只扩展 pg_type/OID 表对整数提升等既有域可复用，但不能恢复已折叠的 bool 参数、varchar/bpchar、timestamptz、JSON/JSONB、数组身份与 typmod；已有 NativeType boolean 测试也表明相同 MySQL 列和值不足以判型。因此选择**复用 NativeType/FieldType 桥接并按必要保真缺口补充规范类型信息**，再由各 wire 适配。这里不命名新 API 或重建完整类型系统；后续先做独立静态/最小往返取证，确定字段、参数声明生命周期和共享消费者，缺证项不开放。方案不改变 MySQL 原有聚合、cast 和错误语义。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 的 Parse、Bind、ParameterDescription、RowDescription、DataRow 支持方向、格式与 NULL 约束；§52.64 [pg_type](https://www.postgresql.org/docs/18/catalog-pg-type.html) 支持 typlen/类型身份含义。OID/typlen 数值核对官方 **REL_18_0** [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)；typmod 核对同 tag 的 [numeric.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/numeric.c) 与 [varchar.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/varchar.c)。§8.1 [Numeric Types](https://www.postgresql.org/docs/18/datatype-numeric.html)、§8.5 [Date/Time Types](https://www.postgresql.org/docs/18/datatype-datetime.html)、§8.14 [JSON Types](https://www.postgresql.org/docs/18/datatype-json.html) 分别支持位宽/精度、时间域区别、JSON 身份区别；[第 8 章](https://www.postgresql.org/docs/18/datatype.html)作为其他族取证索引，非完整 codec 依据。18.6 仍是来源目标；未运行该补丁参考端，REL_18_0 源码不能代替其运行证据。

#### 布尔类型契约

规范 bool 的非 NULL 值域只有 true/false，SQL NULL 表示未知且保留 bool 身份；不存在精度、scale 或长度修饰符（不适用）。布尔列、TRUE/FALSE 字面量与已定型参数都必须保留此身份，不能由整数 0/1、TINYINT 名称或 MySQL Boolean flag 单独推定 PostgreSQL 列语义。现有 Datum 无独立 bool Kind，出口用 IsBooleanFlag 选 OID 16；这只是承载手段，尚未证明声明列和参数的完整身份生命周期。

| 方向 | 目标身份、值与 NULL | 输入输出、错误及 wire | 现状与差距 |
| --- | --- | --- | --- |
| 字面量/列/参数推导 → 规范 bool | TRUE/FALSE 直接定型 bool；NULL::boolean、布尔目标列中的 NULL 保留类型；未知字符串仅在 bool 上下文按 bool 输入解析。Parse OID 16 或 SQL 布尔目标确定参数，OID 0 按上下文推断，无法推断为 42P18。 | ParameterDescription 为 16；结果 OID/typlen/typmod 为 16/1/-1。零行、首行 NULL、后续 NULL 均不改变描述。裸 NULL 不能无条件定型 bool。 | Extended::process 当前对缺参数推导拒绝；parameter 非 NULL bool 转 tp=1，NULL 转 tp=6，不能据此认定仍携带 bool。声明列持久化、Prepare/Execute 保真未验证；第 2 章提供语法/目标身份，本章定型。 |
| PG text 参数/显式文本输入 → bool | 合法表示包括 true/false、yes/no、on/off、1/0，忽略大小写与两端空白；允许唯一前缀如 t、tr、n、of，o 不唯一应拒绝。SQL NULL 用长度 -1，空串不是 NULL。 | 非法文本如空串、o、2、truth、unknown 为 22P02；非法编码目标 22021。以 OID 16 解释，不能先走数值真值转换。 | parameter 当前仅接受大小写不敏感的 t/true/1、f/false/0，未 trim；其他合法拼写缺失。现有测试不覆盖完整词集或有类型 NULL。 |
| PG binary 参数 → bool | 非 NULL 载荷恰好一字节，00 为 false，01–ff 均为 true；这是已声明 OID 16 的接收规则，不授权整数隐式转 bool。NULL 不消费载荷并保留类型。 | boolrecv 接收后规范化；空载荷目标 08P01，多字节剩余数据目标 22P03；长度 -1 为 NULL，其他负长度和不完整 Bind 归工作流 7。单字节没有非法“值”；把 02/ff 当非法值会偏离 PG。 | binary_parameter 当前只收 00/01，其他值/长度均 22P03；目录专用 catalog_binary_parameter 也有限制，不能推为通用支持。上述差距只记录，未修代码。 |
| 规范 bool → PG text/binary 结果 | 非 NULL text 为 t/f，binary 为 00/01；NULL DataRow 长度 -1、无载荷。先校验身份和值域，非法引擎值如带 bool 身份的整数 2 应拒绝，不能掩盖成 true。 | Describe 与执行结果同为 16/1/-1，格式码依 Bind，格式不改 OID。缺类型/非法内部值走明确错误路径（内部不变量目标 XX000），不静默降级 text；SQLSTATE 传输由工作流 7 承接。 | pg_type 使用 B，value_text 只收 1/true/0/false，value_binary 复用文本校验后发单字节；出口 io::Error 不能证明 XX000 已正确传输。完整 bool 列、NULL 与 Describe 一致性未验证。 |
| MySQL 输入/输出 ↔ 共享承载 | 保留原 code/flags、数值域与真值规则；MySQL Boolean flag、逻辑 bool 与整数三者分别记录。PG 布尔类型信息不能把 MySQL TINYINT 全域缩成 bool。 | 沿自身 wire 契约，无 PG OID/SQLSTATE（不适用）；共享 NULL 不改列定义。 | Datum::ToBool 可处理非零数等，保留其 Go/MySQL 契约；不能把它直接接为 PG 隐式转换。现有同列同值测试只证明桥接断言意图，双方言运行回归未验证。 |

转换边界先按方言与上下文决策再复用共享实现：PG 不允许整数直接充当 WHERE 条件（例如 WHERE 1 目标 42804），整数赋给 boolean 列也不能自动走真值转换（类型不匹配目标 42804）。显式 int4→bool 则合法，0→false、非零（含 -1、2）→true；bool→int4 为 false→0、true→1，结果 OID 23。不得把该显式规则泛化为任意整数类型、隐式重载或赋值转换。显式 text→bool 按输入规则，bool→text 的 SQL cast 输出 true/false（OID 25），区别于 bool wire text 的 t/f；有类型 NULL 的这些转换返回目标类型 NULL。其他来源类型的 cast 由后续转换块逐项取证，不套 MySQL 宽松转换；运算符重载及三值逻辑运算归工作流 4。

官方依据（2026-10-23）：PostgreSQL **18** §8.6 [Boolean Type](https://www.postgresql.org/docs/18/datatype-boolean.html) 支持词集、唯一前缀、空白/大小写、字面量与 NULL 推导及 t/f 输出。精确 codec 核对官方 **REL_18_0** [bool.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/bool.c) 的 boolin/boolrecv/boolsend/booltext；长度错误分别核对 [pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c) 的 pq_getmsgbyte 与 [postgres.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/tcop/postgres.c) 的 exec_bind_message 剩余载荷检查。显式转换资格核对同 tag [pg_cast.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_cast.dat) 的 int4/bool castcontext=e，值规则核对 [int.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int.c) 的 int4_bool/bool_int4；无隐式/赋值转换的拒绝目标是据此推导，尚未运行参考端。wire NULL 与描述字段沿用前块 §54.7 依据；18.6 补丁参考端仍未验证。

#### 整数与无符号边界

本块固定精确整数域；整数没有 scale，PG int2/int4/int8 无长度或精度 typmod（-1），十进制显示位数不是声明精度。共享承载必须同时保留声明宽度和 signed/unsigned，不能由当前值是否为正推断。PG 三种常规 SQL 整数均为有符号；目录 oid 的 unsigned 32 位编码不构成通用 unsigned 整数类型。以下为 MySQL 原整数域及 PG 出口目标（不含 Boolean flag 特例）：

| MySQL 身份/code | signed 值域 | unsigned 值域 | 按声明域选择 PG OID/typlen/typmod |
| --- | --- | --- | --- |
| TINYINT/1 | -128…127 | 0…255 | 两者 int2：21/2/-1 |
| SMALLINT/2 | -32768…32767 | 0…65535 | signed int2：21/2/-1；unsigned int4：23/4/-1 |
| MEDIUMINT/9 | -8388608…8388607 | 0…16777215 | 两者 int4：23/4/-1 |
| INT/3 | -2147483648…2147483647 | 0…4294967295 | signed int4：23/4/-1；unsigned int8：20/8/-1 |
| BIGINT/8 | -9223372036854775808…9223372036854775807 | 0…18446744073709551615 | signed int8：20/8/-1；unsigned numeric：1700/-1/-1 |

此表扩宽与 pg_type 当前分支一致，u64 目标选无约束 numeric，不擅自改为 numeric(20,0)。值域保真不等于来源类型可逆：PG int4 不能反推原 MySQL U16，PG numeric 不能反推 U64。反向必须有目标 FieldType/unsigned 身份；缺声明拒绝或保留原规范身份，不从 OID 猜源类型。MySQL 自身输出保留原 code/UnsignedFlag，不因 PG 扩宽改写 MySQL 列定义。

| 方向 | 身份、精确值与 NULL | 错误、wire 及现状差距 |
| --- | --- | --- |
| PG 声明/上下文 → 规范整数及参数描述 | smallint/int2、integer/int/int4、bigint/int8 分别严格为上述 16/32/64 位 signed 全域。参数显式 OID 21/23/20 或上下文定型；OID 0 无法推断目标 42P18。NULL 保留声明宽度而非归 text。 | ParameterDescription 保持输入 OID；结果分别 21/2/-1、23/4/-1、20/8/-1。当前 process 对缺推导报 0A000；parameter(NULL) 为 tp=6，类型生命周期未验证。第 2 章交接类型语法，本章定型。 |
| PG text 参数/显式文本转换 → 规范整数 | 按已定型 signed 域精确解析，覆盖正负号、首尾空白及 PG 整数输入语法；NULL 长度 -1，空串不是 NULL。整数字符串不得经 f64 中转或接受截断数字前缀。 | 合法整数越界为 22003，非法如空串、1x、1.5 为 22P02，非法字符编码 22021。现有 parameter 用 Rust parse i16/i32/i64，将范围错误也映射 22P02，未 trim，不能证明完整 PG 输入语法。PG 18 的进制前缀及下划线规则以 numutils.c 为准，后续补输入用例，不能只凭 Rust parse 推定等价。 |
| PG binary 参数 → 规范整数 | OID 21/23/20 分别恰好 2/4/8 字节网络序、有符号二补码；每个合法长度的位模式都在该整数域内。长度 -1 为 typed NULL，无载荷。 | 接收器读不足目标 08P01；多余载荷目标 22P03，外层 Bind 帧损坏归工作流 7。当前 binary_parameter 将错误长度统一报 22P03，再转共享 little-endian 参数；不存在合法长度的“数值越界位模式”。 |
| 规范整数/MySQL unsigned → PG text/binary 结果 | 先按声明域定型，再按上表整体扩宽；U64 即使值为 0 也保持 numeric。text 输出精确十进制整数；binary int2/int4/int8 按 2/4/8 字节网络序。NULL 为 DataRow 长度 -1。 | value_text 当前直接格式化整数，未按 OID 再检查整数范围；binary 用 parse 检查范围。u64::MAX 的 text/OID 已有断言意图，numeric binary 当前无分支，不能把十进制 ASCII 填进 numeric binary。开放该格式前依赖 numeric 块补齐 codec；未实现路径明确拒绝目标 0A000，禁止回退 float8 或伪装 int8。 |
| PG/共享整数 → 显式窄化、目标 unsigned | 整数之间只在目标域内精确转换；负值转共享 unsigned 目标拒绝，超过 unsigned 上界同样拒绝；这是桥接契约，PG 没有可写作 ::unsigned 的常规整数类型。numeric→U64 仅在有限、无小数且域内的明确桥接路径接收；一般 numeric→PG integer 舍入规则留 numeric/cast 块，不把此桥接规则当 PG 通用 cast。typed NULL 转为目标类型 NULL。 | PG 路径域外目标 22003，不 wrap、饱和或返回截断成功。MySQL ConvertIntToUint 的 AllowNegativeToUnsigned、携带裁剪值的错误和 Context 策略保持原 Go/MySQL 行为，不直接当 PG 转换。共享错误到 SQLSTATE 尚未验证。 |

算术和转换结果的身份必须在 Prepare/Describe 时由已选择的 PG 重载/目标类型确定，执行时不能因某一行溢出而临时提升 OID；例如 int4+int4 的 int4 溢出应 22003，而预先显式转 int8 后计算可得到 int8，不能直接套共享 MySQL 的 BIGINT 推导。`2147483647::int4 + 1::int4`、bigint 最大值加一及各 signed 最小值取负均应有溢出验收；NULL 结果保留同一结果类型。unsigned 来源的预定扩宽是上表的边界策略，不授权改变 PG 算术规则。运算符/聚合完整重载归工作流 4，本块只规定结果类型交接与溢出不改型；显式、赋值、隐式 cast 的资格按后续转换块确定，wire 扩宽不能自动成为 SQL 隐式 cast。

官方依据（2026-10-23）：PostgreSQL **18** §8.1.1 [Integer Types](https://www.postgresql.org/docs/18/datatype-numeric.html#DATATYPE-INT) 定义三种 signed 域及越界错误；§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 定义 NULL、参数 OID 与列描述。精确输入和 codec 核对官方 **REL_18_0** [int.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int.c) 的 int2in/int4in、recv/send、int4pl、int2um/int4um，以及 [int8.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int8.c) 的 int8in/recv/send、int8pl/int8um 和整型窄化；[numutils.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/numutils.c) 的 pg_strtoint16_safe/32_safe/64_safe 区分语法和范围错误；[pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c) 与 [postgres.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/tcop/postgres.c) 的读取/剩余载荷检查支持二进制长度错误目标。错误名称见 PostgreSQL 18 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)。OID 沿前块 pg_type.dat 依据；MySQL 域来自本仓库上下界函数及 test_convert，不以 MySQL 类型名推导 PG 等价。18.6 仍仅为来源目标，未运行该补丁参考端。

#### 精确数值契约

numeric 与 decimal 是 PG 同一精确数值类型（OID 1700、typlen -1），区别于 float4/float8。目标须保存有限精确值、显示 scale、声明 precision/有符号 scale 及特殊值身份；只扩充 OID 或透传字符串不足以证明支持。现有 MyDecimal 与 cast 的 Decimal 容量/错误策略不同，须先验证可保真子域，再决定规范表示补充；不得将 PG 域缩成 MySQL DECIMAL 或用 f64 中转。尚不能承载的合法 PG 域应明确报项目不支持 0A000，记录兼容缺口；PG 自身域外才是 22003，不能混淆。

| 方向 | 值、类型、NULL 与精度目标 | 错误、wire 元数据及当前差距 |
| --- | --- | --- |
| PG 声明/推导 → 规范身份 | numeric(p,s) 的 p=1…1000，s=-1000…1000，可 s>p；numeric(p) 为 s=0；裸 numeric 不强制 scale，有限域上限为小数点前 131072 位、后 16383 位。NULL 保留目标 numeric 身份。 | 无约束 typmod=-1；有约束为 `((p<<16) | (s & 0x7ff))+4`，解码 s 须 11 位符号扩展。非法声明目标 22023。ParameterDescription 仅含 OID，不传 p/s；上下文及结果描述保存修饰符。现有结果一律 -1，不能伪称 constrained numeric。声明语法交工作流 2。 |
| text 参数/文本显式转换 → numeric | OID 1700 下完整解析十进制与指数输入，允许规范所容许的符号/空白；空串、12bad 非法，不能接受数字前缀或回退零。无约束 12.340 保留显示 scale；有约束先舍入再检查整数域。NULL 长度 -1，无载荷；空载荷不是 NULL。 | 非法语法 22P02，非法字符编码 22021，数值范围/舍入后超域 22003。parameter 当前调用 FromString 后保留原字节，所有解析错误映为 22P02；不是完整 PG 输入/范围分类证据。 |
| binary 参数 → numeric | 网络序 16 位字段依次为 ndigits（unsigned 计数）、weight（signed）、sign、dscale，后随 ndigits 个 base-10000 digit（0…9999）；有限值为符号乘各 digit×10000^(weight-i)。长度应为 8+2×ndigits；NULL 在外层长度 -1 表示。 | sign 为 0000/4000/C000/D000/F000（正/负/NaN/+∞/-∞），dscale 为 0…16383；它是显示 scale，不是带符号的声明 scale。接收有限数先截去 dscale 隐藏位，再应用 typmod 舍入/范围。非法 sign/digit/scale 22P03；读取不足 08P01，额外载荷 22P03（沿前块消息边界）。当前非 NULL 1700 无 binary 分支，报 0A000；NULL 能走公共分支不证明身份连续。 |
| 规范 numeric/MySQL DECIMAL/U64 → PG text/binary 结果 | Decimal 原身份保留，U64 沿前块输出无约束 numeric；text 精确输出十进制及应保留的小数零，binary 转换 base-10^9 到 base-10000，不能发送 MyDecimal::ToBin 格式或 ASCII。两格式解码后的值与显示 scale 一致，不要求载荷相同。NULL 为 DataRow 长度 -1。 | OID 1700/typlen -1，typmod 随声明或表达式定型，text/binary 不改变类型；零行、全 NULL 同样 Describe。value_text 当前透传值文本，未校验 numeric 域；value_binary 无 1700；encode_formats 固定 typmod -1。未开放 codec 明确拒绝目标 0A000，不降为 float8/int8，io::Error 到 SQLSTATE 尚未验证。 |
| numeric → 目标精度/整数/共享 DECIMAL | precision/scale coercion 的半值远离零；负 scale 在小数点左舍入，s>p 限制小数点后前导零。numeric→PG signed integer 先按整数舍入再检查对应域；整数→numeric 精确保值。typed NULL 转为目标类型 NULL。 | 正负超域均 22003，不裁剪、wrap 或仅警告成功；如 99.95/-99.95→numeric(3,1) 均因进位超域失败。截断只用于显式截断语义或 binary 接收 dscale 规则，不能代替 cast 舍入。共享 unsigned 的有限无小数桥接仍沿前块；MySQL warning/裁剪原行为独立保留。显式/赋值/隐式资格交本章转换块，运算重载交工作流 4。 |

特殊值单列：目标规范身份须区分 NaN、Infinity、-Infinity 与 SQL NULL；NaN 可进入 constrained numeric，无穷只可进入无约束 numeric，进入有限精度声明目标 22003。输出 text 使用 NaN/Infinity/-Infinity，binary 使用上述专属 sign。MyDecimal 的符号与有限 word 布局没有特殊值标记；Value::Float 的特殊值输出分支或 Value::Text 透传不能证明 numeric 特殊值已可表示、转换或求值。支持前明确 0A000，禁止把它当普通字符串成功；特殊值比较/算术由工作流 4 承接。

官方依据（2026-10-23）：PostgreSQL **18** §8.1.2 [Arbitrary Precision Numbers](https://www.postgresql.org/docs/18/datatype-numeric.html#DATATYPE-NUMERIC-DECIMAL) 支持精度、scale、舍入和特殊值目标。精确布局、typmod、错误及转换以官方 **REL_18_0** [numeric.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/numeric.c) 的 make_numeric_typmod/numeric_typmod_scale、numerictypmodin、numeric_in/out、numeric_recv/send、apply_typmod/apply_typmod_special、numeric_int4 为据；NULL/描述与外层载荷沿前块 §54.7、pqformat.c/postgres.c 来源。以上是规范和源码阅读证据，非参考端实测；来源指定的 18.6 补丁仍未验证。

#### 浮点数值契约

real/float4 与 double precision/float8 是两个规范身份，目标分别为 IEEE 754 binary32/binary64（有效二进制精度 24/53 位），PG OID/typlen/typmod 分别 700/4/-1、701/8/-1。有限最大绝对值为 (2−2^-23)×2^127、(2−2^-52)×2^1023；最小正规正数为 2^-126、2^-1022，最小次正规正数为 2^-149、2^-1074。这些是二进制域边界，不以文档概述的十进制量级代替；近似类型不能承诺十进制精确计算。[PostgreSQL 18 §8.1.3](https://www.postgresql.org/docs/18/datatype-numeric.html#DATATYPE-FLOAT) 为位宽、精度、特殊值和溢出目标依据。

推导消费工作流 2 的类型节点：real/float4 固定为 float4，double precision/float8 为 float8；float(p) 的 p 为二进制精度，1…24 选择 real、25…53 选择 double precision，无 p 选择 double precision。p=0 或 p=54 由声明解析拒绝 22023（REL_18_0 [gram.y::opt_float](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/parser/gram.y)），不把 MySQL FLOAT(M,D) 当 PG 声明。裸 `1.5`、`1e0` 按 PG 数值常量规则初始为 numeric，不能因带小数点或指数直接推导 float8；显式转换和上下文才能改变类型（[18 §4.1.2.6](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-SYNTAX-CONSTANTS-NUMERIC)）。OID 0 参数按上下文定型，无法确定时目标 42P18；ParameterDescription 确定 OID，格式码不决定宽度。

| 方向 | 值域、精度、NULL 与错误目标 | wire 身份、现状及差距 |
| --- | --- | --- |
| PG text 参数 → 规范浮点 | 按目标宽度直接解析十进制/指数，允许首尾空白及大小写不敏感 NaN、±Infinity/±Inf；拒绝空串、尾随垃圾为 22P02，编码错误 22021。有限文本溢出为无穷或非零下溢为零时 22003；可表示次正规数成功。float4 不先解析 float8 再窄化，以免二次舍入。 | OID 700/701 决定 f32/f64；长度 -1 为 typed SQL NULL，无载荷，空串不是 NULL。当前 Rust parse 的词法、超域分类与 PG 尚不等价，解析失败统一 22P02；NULL 参数仅 tp=6，身份连续性未验证。 |
| PG binary 参数 → 规范浮点 | 载荷恰为 4/8 字节的 IEEE 位型，含次正规、±0、NaN、±∞；接收本身无十进制舍入，不能将无穷当有限超域拒绝。过短目标 08P01、额外字节 22P03，消息截断仍归工作流 7。 | 网络大端；外层 -1 为 NULL，0 长度非法。当前 binary_parameter 已有网络序转换，但短/长均映为 22P03；保留位宽、零符号与特殊值的全链证据未验证。 |
| 规范浮点 → PG text 结果 | 先以规范身份校验并取得目标宽度的值；默认 extra_float_digits=1 时按该宽度输出可回读同值的最短十进制，不打印 f32 扩宽后的 f64 噪声。特殊值文本 NaN/Infinity/-Infinity，零符号须可观察；SQL NULL 长度 -1。 | 两格式均固定 700/4/-1 或 701/8/-1；当前 pg_type 有 4→700、5→701，value_text 共用 f64 to_string，不能证明 float4 或 PG 精确拼写。会话非默认 extra_float_digits 的状态/设置归工作流 5，不能将其有损文本仍标为无损往返。 |
| 规范浮点 → PG binary 结果 | 按身份输出 4/8 字节网络序，有限值和 ±0 保持目标宽度位型；±∞保持符号，NaN 按 NaN 类别验收，不承诺 NaN payload/signaling 位跨文本或运算保存。NULL 无载荷。 | 当前 value_binary 经文本重解析，不能据 1.5 测试推断所有位型可逆；内部未窄化 float4 不得在出口静默变无穷/零，已开放域错误 22003，缺保真表示/codec 时明确 0A000，工作流 7 承接 SQLSTATE 传输。 |
| float4 ↔ float8 / 数值 cast | float4→float8 有限值精确扩宽；float8→float4 按目标宽度舍入，有限溢出、非零变零拒绝 22003，次正规非零成功；原有无穷/NaN 合法传递。typed NULL 保留目标身份。浮点→整数先按参考平台 rint 舍入再检查域，NaN/无穷拒绝 22003；numeric 半值远离零规则不能套用。 | 目标 OID 由 cast 预先确定，不能按值动态提升。显式/赋值/隐式资格交本章转换块核定，算术/重载/round 归工作流 4；不把本表当完整隐式转换矩阵。MySQL 参数和结果维持自身 FLOAT/DOUBLE code、flags 与告警契约，不套 PG 错误；共享 f64 容器不是双方言等价证据。 |

特殊值须区分 SQL NULL：NaN 是非 NULL 的浮点值，PG 比较语义中 NaN 等于 NaN 且大于其他值，±0 数值相等但 codec 验收分别检查符号；无穷、NaN、零的比较与算术由工作流 4 拥有。本块不要求文本往返保存 NaN payload；binary 直通是否保存其位型须另采证，不能将全部 NaN 位串判为非法。舍入验收记录参考端平台与模式，常见最近偶数规则须有具体半值断言，不能用统一 epsilon 淹没 float4/float8 差异。

官方源码依据为 **REL_18_0** [float.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/float.c) 的 float4in_internal/float8in_internal、float4out/float8out、float4recv/send、float8recv/send、ftod/dtof、dtoi4：分别核对直接目标宽度解析、输入/范围错误、输出及窄化和整数转换；二次舍入反例取其注释 `7.038531e-26`。网络位型与不足读取见同 tag [pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c) 的 pq_sendfloat4/8、pq_getmsgfloat4/8、pq_copymsgbytes；额外载荷错误沿前块消息边界来源。输出配置以 PostgreSQL **18** [§19.11.2 extra_float_digits](https://www.postgresql.org/docs/18/runtime-config-client.html#GUC-EXTRA-FLOAT-DIGITS) 为据。查阅日期 2026-10-23；以上均为规范阅读，18.6 补丁和实际参考端行为仍未验证。

#### 文本类型契约

本块定义通用 text、varchar、SQL char（内部 bpchar），与目录单字节 `"char"` 区分。长度按字符计数，wire 长度按字节计数；UTF-8 下不按字素簇或视觉宽度截断，不自动 Unicode 规范化。数值精度/scale 不适用，约束是字符数 n。无长度 varchar 保持无约束身份，SQL char 省略长度按 1；无长度 bpchar 是独立的无约束、尾空格不显著形式，不能误套 char(1)。声明长度 n 的 PG 18 合法域为 1..10485760；不合法声明目标 22023，由工作流 2 保留类型语法并交本章校验。

| 方向 | 值、NULL 与错误目标 | 身份、元数据及现状差距 |
| --- | --- | --- |
| PG text 参数 → 规范文本 | 按已确定 OID 完整解码；空载荷为空串，长度 -1 为 SQL NULL，无载荷；两者不得合并。UTF-8 基线拒绝 NUL、非法序列为 22021，不替换字符或截断至 NUL。 | 25/1043/1042 各自保留，OID 0 仍交定型，不猜 text。ParameterDescription 只有 OID，无 typmod 字段；SQL 目标的长度约束不能因此丢失。当前输入校验存在，身份/长度接线未证明。 |
| PG binary 参数 → 规范文本 | 载荷是协商编码的文本字节，不带内部 varlena 头、不以 NUL 终止；空载荷合法，NULL 仍为 -1。与 text 参数同样验证编码/NUL，binary 不绕过长度约束。 | 格式 1 不改变类型，现有路径复用 parameter；并非任意 bytes/bytea。UTF-8 之外的协商和转码归工作流 5、7，未开放编码明确 0A000，不能假称已转码。 |
| 规范文本 → PG text/binary 结果 | 两格式都输出已定型值的文本字节；char(n) 的补空格保留，varchar/text 的尾空格保留；NULL 长度 -1，非 NULL 空串为 0。非法内部编码/NUL 在发布结果前拒绝，目标 22021；缺身份/codec 的能力门禁为 0A000。 | text 为 25/-1/-1；varchar(n)、char(n) 为 1043、1042，typlen 均 -1，typmod=n+4；无约束 typmod=-1。Prepare/Describe/Execute、零行及全 NULL 均一致，不按首行猜。当前普通 text fallback、固定 -1 和 io::Error 尚不能证明身份与 SQLSTATE 传输。 |
| 赋值/长度 coercion 与显式 cast | 向 varchar(n)/char(n) 赋值超长且超出部分含任何不是 U+0020 的字符时 22001；仅多出的 U+0020 可裁去。显式 cast 可截到 n 字符且不切断多字节编码。char(n) 短值补 U+0020，varchar 不补；char 转 text/varchar 去除尾部补空格，text/varchar 不通用 trim。typed NULL 保留目标身份。 | 目标 OID/typmod 由声明或表达式定型决定。隐式/赋值/显式资格交本章转换块，不由 codec 擅自截断；尚无 PG 长度 coercion 完整证据。通用字符值不承诺数值转换、排序或匹配算法。 |
| MySQL 参数/结果 ↔ 共享值 | 保留其自身 charset、collation、声明长度、严格/非严格 sql_mode 与 CHAR 尾空格规则；字符长度和协议字节宽度分开。NUL、非法序列、截断的错误/告警按 MySQL 参考端单独验收，不强套 PG 22021/22001。 | 保留 VARCHAR/VAR_STRING/STRING code、flags、charset 与长度元数据，不将 PG OID 或 n+4 写回 MySQL。共享 KindString、名称相似或 charset=63 均不是双方言语义等价证据；不在本块重写既有 MySQL 路径。 |

规范依据（查阅于 2026-10-23）：[PostgreSQL 18 §8.3 Character Types](https://www.postgresql.org/docs/18/datatype-character.html) 支持字符长度、补空格、赋值与 cast 差别、尾空格及 NUL 禁令；[REL_18_0 varchar.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/varchar.c) 的 anychar_typmodin、bpchar_input、bpcharrecv/bpcharsend、varchar/varcharrecv/varcharsend 支持 typmod 与文本载荷边界；[REL_18_0 varlena.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/varlena.c) 的 textrecv/textsend 支持无内部头的文本字节编码。[PG 18 §54.7 Bind/DataRow/RowDescription/ParameterDescription](https://www.postgresql.org/docs/18/protocol-message-formats.html) 支持 -1 NULL、字节长度及元数据字段；[PG 18 Appendix A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 支持 SQLSTATE 分类。固定源码标签是 18.0 取证，不等于已在目标 18.6 运行或证明补丁版全部一致。

#### 字节类型契约

规范 bytea 为有序原始字节序列，每字节 0–255，允许 NUL、非 UTF-8、前导/尾随零；长度按字节计，字符精度、scale、collation 不适用。复用引擎 bytes 与 binary charset 的 string/blob 家族时须保留 bytea 身份，不因值可打印就改为 text，不从 BLOB/BIT 名称推导 PG 等价。PG 参数 OID 17，结果 OID/typlen/typmod=17/-1/-1；MySQL 独立验收 type code、charset/flags、长度，保留 Go 原行为。资源上限须实际取证，不能从变长类型或 i32 帧长度承诺已支持近 1GB；超限明确拒绝，禁止截断。

规范依据：[PostgreSQL 18 §8.4、8.4.1、8.4.2](https://www.postgresql.org/docs/18/datatype-binary.html) 支持字节域、hex/escape 语法与 bytea_output；[官方 REL_18_0 varlena.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/varlena.c) 的 byteain/byteaout 支持 escape 错误及输出，bytearecv/byteasend 支持 raw binary。核查日期 2026-10-23；源码固定为 18.0，文档为 18 主版本，不构成来源指定 18.6 的发布或运行验证。

| 方向 | 目标值、NULL 与错误 | 当前差距与元数据 |
| --- | --- | --- |
| text 参数→bytes | Bind 实际字符的 hex 以 `\x` 开头，数字大小写皆可，字节对间可有空白，不能拆开一对；`\x` 为零字节。escape 接受普通字符、`\\` 为 5c、三位八进制 `\000`/`\377` 为 00/ff；空串也为零字节。非法数字/奇数半字节/非法 escape 拒绝 22P02；UTF-8 文本载荷非法编码/NUL 拒绝 22021，但解码后的 00/ff 合法。 | 当前 hex-only/无空白为兼容缺口，不能把合法输入被拒绝当作目标通过。NULL 长度 -1 不解析，仍保留 OID 17。SQL 字面量先经过工作流 2 的引号/反斜线解析，不能把 SQL 拼写直接当 Bind 字符。 |
| binary 参数→bytes | 逐字节保留，无 hex/escape 解析、字符转码、字节序转换、varlena 头或终止 NUL；长度 0 合法，NULL 为 -1。任何字节组合本身都不是非法 binary bytea。 | binary_parameter 可直接复制，内核往返缺证；不能套固定宽度错误或以 22P03 拒绝任意字节。帧截断/长度不符由工作流 7 拒绝 08P01，资源限制另记。 |
| bytes→text 结果 | 默认 bytea_output=hex：`\x` 加每字节两位小写数字，空序列输出两个字符 `\x`。escape 模式将反斜线输出两次、非打印字节输出三位八进制、打印 ASCII 原样；回读保留同一字节序列。 | 当前只输出 hex，escape 配置/输出缺证，会话状态归工作流 5。DataRow 长度按输出字节计（hex 为 2+2n），不含 C 结束符；SQL NULL 只写 -1，无载荷。 |
| bytes→binary 结果 | n 个原始字节原样发送，含 NUL/ff；空序列长度 0，NULL 长度 -1。格式选择和 bytea_output 不改变类型或值。 | Value::Bytes 可复用；禁止经 BinaryLiteral::ToString、from_utf8_lossy、替换字符、去前导零或数值转换。Describe/Execute、零行、全 NULL 均保持 17/-1/-1；局部结果断言不证明全链。 |

转换边界：bytea 同型转换保留全部字节与 typed NULL；bytea 的文本输出表示和字符解码是不同契约，不能隐式使用有损 UTF-8 或把 text 直接改标 bytea。text↔bytea 的显式/赋值/隐式资格交本章转换块核对 PG cast；未开放路径明确 0A000，不借 MySQL HEX/整数转换猜测兼容。字节比较与运算交工作流 4，不能复用去前导零的 BinaryLiteral::Compare 推断等价。

#### 日期类型契约

日期是独立 date 身份，无时刻、时区或小数秒精度；目标采用前推 Gregorian 日历。官方 [PostgreSQL 18 §8.5、§8.5.1.1、§8.5.1.4、§8.5.2](https://www.postgresql.org/docs/18/datatype-datetime.html) 支持日历、输入/输出及特殊词目标：基线固定 UTF-8、DateStyle=ISO,YMD，普通输出 YYYY-MM-DD，BC 保留 BC 后缀；MDY/DMY 的 `1/8/1999` 分别为 1 月 8 日/8 月 1 日，不能套 MySQL 短年/分隔符规则。`epoch` 为 1970-01-01；now/today/yesterday/tomorrow 在读取时解析为普通日期，依赖事务时间和会话时区，不能误作每次执行重新计算的动态 date。±infinity 是独立非有限值，非 NULL、非最大/最小有限日；allballs 仅 time，不能作为 date 成功输入。

精确有限域按官方 18 分支 [timestamp.h](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/include/datatype/timestamp.h)::IS_VALID_DATE/POSTGRES_EPOCH_JDATE：日序 -2451545..2145031948（含端点），对应 Julian day 0 至 5874897-12-31；下端按前推 Gregorian 为 4714-11-24 BC，区别于手册表中概略年份 4713 BC。BC/AD 无外部年份 0，内部天文年不能直接以 0000 文本输出。[date.h](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/include/utils/date.h)::DateADT/DATEVAL_NOBEGIN/DATEVAL_NOEND 定义 i32 及 -2147483648/+2147483647 两个特殊哨兵；其余超有限域整数必须拒绝，不能全部当日期。

二进制按官方 18 分支 [date.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/utils/adt/date.c)::date_in/date_out/date_recv/date_send：四字节有符号网络序、单位日、基准 2000-01-01，零即该日，-1 为 1999-12-31，+1 为 2000-01-02；不是 Unix 秒、微秒或 MySQL 年月日结构。两特殊哨兵分别对应文本 -infinity/infinity。该链接为读取于 2026-10-23 的可变 REL_18_STABLE，提供 18 分支格式证据，未锁定 18.6 构建或运行参考端；实施时须记录实际参考提交。

| 方向 | 类型、值域、NULL 与 wire 目标 | 错误和现状差距 |
| --- | --- | --- |
| text 参数→规范 date | 已定型 OID 1082 按 PG 日历/DateStyle 解码；日期身份跨桥接保留，不能只变字符串或 MySQL 宽松日期。NULL 长度 -1，无载荷且仍为 date；空串是非法非 NULL。 | 非日期词/空串 22007；非法月日、零日期和有限范围越界 22008；非法编码/NUL 22021。当前 chrono 单格式及 1..9999 不能证明完整 PG 输入。 |
| binary 参数→规范 date | 四字节解日序及哨兵，域内与对应 text 相同值、日精度；NULL 同上，不进入长度检查。 | 当前非 NULL 完全未开放。不足四字节目标 08P01，额外字节 22P03，合法四字节但越日期域 22008；帧截断另为 08P01。不能把空载荷当零日。 |
| 规范 date→text 结果 | 列 OID/typlen/typmod=1082/4/-1；输出随已固定 DateStyle，有限日期/BC/特殊值可回读。SQL NULL 长度 -1，不输出空串或特殊词。 | 当前粗校验漏非法月日，不能原样透传。MySQL 0000-00-00、2024-00-01、2024-01-00 及非法实际日显式拒绝；内部值不合法目标 22008，不静默归一化。 |
| 规范 date→binary 结果 | 同一列身份 1082/4/-1，四字节日序或特殊哨兵；与文本结果表达同一日期，NULL 仅 -1 长度。 | chrono 和现有 MySQL 表示不能证明 BC、超 9999 或非有限值保真；禁止截断、wrap、借用零日期或降为 text OID。 |

错误分类依据官方 18 分支 [datetime.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/utils/adt/datetime.c)::ValidateDate/DateTimeParseError 与 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)；协议长度分类沿本章共同契约，消息边界由工作流 7 负责。当前结果编码使用 io::Error，扩展执行可统一包装 0A000，尚不能证明精确 SQLSTATE。PG 合法扩展年份/非有限值若共享表示暂不能承载，过渡拒绝须明确 0A000 并登记兼容缺口；这不等于完成 PG 全域目标，也不能错误报告用户输入超 PG 范围。

类型推导以声明 DATE、typed literal/cast 或参数上下文定型为 date；普通字符串外观不能决定 OID，未知参数交本章映射/转换块。无 date(p) 小数精度，typmod=-1，不把 MySQL fsp/显示宽度充作 PG typmod。显式 text↔date 按日期输入/输出契约；date↔timestamp/timestamptz 的资格与时间/时区转换交对应类型及转换块，不能按天乘微秒自动推导隐式转换。MySQL 既有宽松解析、零日期及 sql_mode 告警独立保留；从 MySQL 结果桥接 PG 时逐值检查合法性，拒绝不改变原 MySQL 行为。

#### 日内时间类型契约

time 单独使用即 time without time zone，保留日内身份，不携带日期或偏移。目标有限域为含端点的 0..86400000000 微秒，末端独立表示 24:00:00，不能 wrap 成零点。官方依据（2026-10-23 查阅）为 [PostgreSQL 18 §8.5 表 8.9、§8.5.1.2、§8.5.1.4 与 §8.5.2](https://www.postgresql.org/docs/18/datatype-datetime.html)：支持日内值、输入形式与输出；普通 time 输入中合法时区说明被忽略，不能因此定型为 timetz，也不能按 TimeZone 移动已存墙钟时间。ISO 基线接受 04:05、040506、04:05 PM 等官方形式，输出 HH:MM:SS 加必要小数位；allballs 为零点。命名时区涉及日期的解析要求由会话工作流协作，不以删除字符串后缀替代校验。

精度为秒后 0–6 位，未声明 typmod=-1，声明 time(p) 的 typmod 直接为 p，不加 4。先按 PG 输入规则归为微秒，再按目标 p 舍入，不能简单截断或把输出补零宽度当精度。官方 18 分支 [date.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/utils/adt/date.c)::anytime_typmod_check/AdjustTimeForTypmod/time_recv/time_send 为精确依据：负精度拒绝 22023，大于 6 告警 22023 并按 6；非负微秒以半单位加偏移的整数方式舍入，日末可进位到 24:00:00。binary 为八字节有符号网络序日内微秒，不是 Unix 时间或 MySQL duration 结构，接收范围在舍入前校验。该链接为可变 REL_18_STABLE，未锁定 18.6 参考构建；后续实施须记录实际提交及对照版本，不把本轮阅读当实测。

| 方向 | 类型、值域/精度、NULL 与 wire 目标 | 错误和现状差距 |
| --- | --- | --- |
| text 参数→规范 time | 已定型 OID 1083，按 PG 日内输入解析并应用 SQL 目标 p；ParameterDescription 无 typmod 字段，不丢 SQL 约束。NULL 长度 -1、无载荷仍保留 time 身份；空串不是 NULL。 | 非法文本/空串 22007，分秒/小时越界 22008，非法 UTF-8/NUL 22021；当前单一 chrono 格式及非整微秒拒绝不等于 PG 输入舍入。 |
| binary 参数→规范 time | 恰八字节 i64 微秒，0 与 86400000000 均合法，按目标 p 舍入，与 text 表达同一规范值。NULL 同上，不走长度检查。 | 负数、86400000001 及 i64 两端拒绝 22008；不足八字节 08P01，额外字节 22P03，帧声明不符 08P01。当前非 NULL 未开放。 |
| 规范 time→text 结果 | 1083/8/p 或无声明 -1；合法值规范输出、可回读，24:00:00 保留；NULL 为长度 -1。先验证完整时分秒和微秒域。 | 不能仅验证小时后透传非法分钟/秒；非法内部日内值目标 22008，缺表示保真能力明确 0A000 并记兼容缺口。现有 io::Error 尚不证明准确 SQLSTATE。 |
| 规范 time→binary 结果 | 同一列身份及 typmod，八字节网络序微秒；NULL 仅 -1 长度，不能编码为数值零。Describe 与 Execute 不随首行值/格式改型。 | 当前 binary 经文本再解析，24:00 与精度保真缺口须补证；不能静默降为 text OID 或按天取模。 |

文本范围与错误依据另见官方 18 分支 [datetime.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/utils/adt/datetime.c)::DecodeTimeOnly/time_overflows/DateTimeParseError 和 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)；消息长度及恢复沿本章共同规则交工作流 7。显式 text↔time 依输入/输出契约；time(p) coercion 保留类型，时间戳取 time、time 与 interval 的转换资格交对应块和转换块，不能因同为 i64 自动允许隐式转换。

MySQL 参数/结果仍按自身 TIME code=11、符号、Fsp、天数及 sql_mode 解释，不套 PG SQLSTATE，也不改 Go 原行为。共享 duration→PG time 仅在身份和精度可保真、值在日内域时显式桥接；负值 -00:00:01、25:00:00、838:59:59 均明确 0A000 拒绝，不取绝对值、不取模、不截断或自动提升 interval。反向 PG time→MySQL TIME 须有显式桥接并保留微秒/24:00，typed NULL 仍需原身份；没有 PG 类型身份时不能从同名 TIME 或字符串外观推导等价。PG time with time zone（timetz，OID 1266）不在来源枚举类型中，本块不新增其支持，定型/codec 请求须明确 0A000；普通 time 文本忽略合法偏移与支持 timetz 是两项不同契约。

#### 无时区时间戳契约

timestamp 单独使用即 timestamp without time zone，表达日期加墙上时间，不是 UTC 瞬间；类型由声明、typed literal/cast 或参数上下文决定，不能检查字符串偏移或列名后改成 timestamptz。官方行为依据为 [PostgreSQL 18 §8.5 表 8.9、§8.5.1.3–4、§8.5.2](https://www.postgresql.org/docs/18/datatype-datetime.html)（2026-10-23 查阅）：合法时区说明在已定型的无时区输入中被忽略，不移动日历字段；DateStyle 决定歧义输入与输出。固定 ISO 基线，输出日期和时分秒及必要小数，BC/AD 无外部零年；支持其他官方输入形式时须按配置差分，不能只删除后缀。

精确表示依据锁定官方 **REL_18_0（18.0）** [timestamp.h](https://github.com/postgres/postgres/blob/REL_18_0/src/include/datatype/timestamp.h)::IS_VALID_TIMESTAMP/MIN_TIMESTAMP/END_TIMESTAMP：有限微秒域为 `[-211813488000000000, 9223371331200000000)`，基准 2000-01-01 00:00:00；下端为 Julian day 0（前推 Gregorian 4714-11-24 BC），上端最后微秒为 294276-12-31 23:59:59.999999。手册概略年份不代替精确日序；共享 MySQL 位域与 chrono 不能证明覆盖此域。i64 最小/最大另表示 -infinity/infinity，不是有限端点或 NULL。

[同版本 timestamp.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/timestamp.c)::timestamp_in/out/recv/send、anytimestamp_typmod_check、AdjustTimestampForTypmod 定义文本和二进制边界：binary 恰八字节有符号网络序微秒，不是 Unix 秒、纳秒或 MySQL 日期结构；无声明 typmod=-1，timestamp(p) 为 p（0–6，不加 4）。声明负 p 拒绝 22023，超过 6 告警 22023 并收敛到 6；输入先归微秒，再按目标 p 舍入，typmod 半单位按相对 PG epoch 的正负向远离零舍入，不能对负值套正值取整或直接截断。超过六位输入按 PG 解析舍入，不沿用现有拒绝；进位可跨日/年。这些来源未证明 18.6 构建或本地功能通过，实施时需锁定参考构建。

| 方向 | 类型、精度、NULL 与 wire 目标 | 错误与当前差距 |
| --- | --- | --- |
| text 参数→规范 timestamp | OID 1114，按 PG 日期时间及 DateStyle 解析，应用 SQL 目标 p；ParameterDescription 不携 typmod，SQL 约束仍需保留。NULL 长度 -1、无载荷仍有类型，空串不等于 NULL。 | 空串/garbage 为 22007；非法日历、零日期与超范围 22008；UTF-8/NUL 错误 22021。当前固定格式、年限及非整微秒拒绝不符合完整目标，temporal 的错误分类也未验证。 |
| binary 参数→规范 timestamp | 八字节 i64 按有限域或两个特殊哨兵解释；应用 SQL 精度约束，与 text 同规范值；NULL 不走载荷长度/范围检查。 | 不足八字节 08P01，额外字节 22P03，帧声明不符 08P01；其余超有限域整数 22008。当前非 NULL 未开放。 |
| 规范 timestamp→text 结果 | OID/typlen/typmod=1114/8/p 或 -1；输出可回读的墙上时间，保留 BC、扩展年份、微秒或 infinity；NULL 仅长度 -1。 | 完整校验日历和时间，非法内部值 22008；合法 PG 值无法保真承载时明确 0A000 并登记兼容缺口，禁止裁到 9999、epoch、NULL 或自动改 1184。当前粗查日期不足，io::Error 不证明 SQLSTATE。 |
| 规范 timestamp→binary 结果 | 同一列身份及 typmod，八字节网络序微秒或特殊哨兵；NULL 无载荷，不编码为零（零是 PG epoch）。Describe 与 Execute 结果格式切换不改身份。 | 当前经文本再解析的 chrono 路径不证明全域、特殊值与舍入保真；不能以 text 能透传为 binary 可用证据。 |

错误名参照 [PostgreSQL 18 附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)，长度与消息恢复沿共同契约交工作流 7。epoch 输入归 1970-01-01 00:00:00；infinity/-infinity 独立保存；now/today/tomorrow/yesterday 在读入时按事务时间及会话日期归具体值，Prepare 常量和 Bind 参数的读入时机分别验收。相对词、DateStyle 的状态归工作流 5；精确输入分类可对照官方 REL_18_0 [datetime.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/datetime.c)::DecodeDateTime/DateTimeParseError，不能将所有解析失败合为 22P02。

MySQL DATETIME(code=12) 与 TIMESTAMP(code=7) 必须保留 NativeType、Fsp、sql_mode 和会话时区证据：前者按自身日历字段，后者在既有会话/存取转换后可能显示不同字段，不从显示值推断原始瞬间。MySQL→PG 仅在显式桥接策略和来源语义已定、值/精度可保真时输出 1114；缺少会话转换前提明确 0A000，零/非法日期两格式均 22008，不补时区、不暗转 1184。反向 PG→MySQL 显式桥接须校验目标类型范围、Fsp、零日期政策与 TIMESTAMP 会话转换；BC、扩展年份或非有限值无法承载时 0A000，不静默截断，也不修改原 Go/MySQL 错误与告警。显式 text↔timestamp 依上述输入/输出；date→timestamp 的午夜、timestamp→date 的去时间及与 timestamptz 互转的时区前提交转换块/下一类型块确定资格，不因共用数值单位允许隐式 cast。

#### 带时区时间戳契约

timestamptz 是 timestamp with time zone 的别名，身份表达瞬时，不保存原时区名称或输入偏移拼写。输入有偏移时据此归一，无偏移时依当前 TimeZone；输出按会话时区显示。类型必须先由声明、cast 或参数上下文确定，不能靠字符串外观改型；DateStyle 影响输入/显示。依据为 [PostgreSQL 18 §8.5.1.3、§8.5.2–3](https://www.postgresql.org/docs/18/datatype-datetime.html)（2026-10-23 查阅）。例如 `2000-01-01 00:00:00+00` 与 `2000-01-01 08:00:00+08` 同值；会话从 UTC 改为 Asia/Shanghai 只改变显示，不改变存量瞬时。

夏令时歧义按 [PostgreSQL 18 附录 B.2](https://www.postgresql.org/docs/18/datetime-invalid-input.html)：向前跳跃缺失时间采用转换前偏移，向后重复时间采用转换后偏移；不能统一拒绝或取 chrono 默认。America/New_York 的 `2018-03-11 02:30` 解释为 UTC 07:30，显示 03:30-04；`2018-11-04 01:30` 解释为 UTC 06:30，显式 -04 则为 UTC 05:30。时区数据库版本必须随证据记录；数值偏移无此歧义。

规范瞬时以 UTC 的 2000-01-01 午夜为微秒基准；有限域 `[-211813488000000000, 9223371331200000000)`，两端及 BC/扩展年份承接前块精确日序，范围按归一后瞬时检查，不能仅查本地显示年份。i64 最小/最大分别为 -infinity/infinity，均非 NULL。依据锁定官方 **REL_18_0（18.0）** [timestamp.h](https://github.com/postgres/postgres/blob/REL_18_0/src/include/datatype/timestamp.h)::TimestampTz/IS_VALID_TIMESTAMP/TIMESTAMP_NOBEGIN/TIMESTAMP_NOEND；这不是 18.6 参考端运行证据。

身份元数据按同版本 [pg_type.dat](https://github.com/postgres/postgres/blob/REL_18_0/src/include/catalog/pg_type.dat) 为 OID 1184、typlen 8；typmod 未声明 -1，声明 p 为 0–6 的 p，不加 4。ParameterDescription 只有 OID，SQL 目标精度仍须保留。精度处理依据 [timestamp.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/timestamp.c)::timestamptz_in/out/recv/send、anytimestamp_typmod_check/AdjustTimestampForTypmod：与前块共用微秒和 p 舍入规则，半单位按 epoch 两侧远离零，进位可跨日；负声明 p 拒绝 22023，超过 6 告警并收敛到 6。合法七位输入不能照搬当前参数拒绝。

| 方向 | 值、精度、NULL 与 wire 契约 | 错误与现状差距 |
| --- | --- | --- |
| text 参数→规范 timestamptz | 已定型 1184，解析日期/偏移或会话时区后归一并应用 SQL p；NULL 长度 -1 无载荷，保留类型；空串不是 NULL。 | 空串/语法错误 22007，非法日历、零日期、归一溢出 22008，UTF-8/NUL 22021；当前非 NULL 1184 为 0A000，完整 PG 解析与错误未验证。 |
| binary 参数→规范 timestamptz | 恰八字节有符号网络序微秒或两个哨兵；载荷没有时区字段，不能再加减会话偏移；应用 SQL p，NULL 跳过载荷解析。 | 短载荷 08P01、多余字节 22P03、消息帧不符 08P01；非哨兵超有限域 22008，并按参考 recv 检查能否转换显示。当前非 NULL 1184 无分支。 |
| 规范 timestamptz→text 结果 | 1184/8/p 或 -1；按 TimeZone/DateStyle 输出带偏移的可回读文本，特殊值输出 infinity/-infinity；NULL 仅长度 -1。 | 非法内部瞬时 22008；合法 PG 值无法承载或所需会话信息缺失时明确 0A000 并登记缺口，不伪造 UTC、不冒充 1114。现有 code=7/12 映射不能证明此方向。 |
| 规范 timestamptz→binary 结果 | 同一身份与精度，八字节网络序瞬时，两格式共享规范值；NULL 无载荷；时区变化不改变字节。 | 共用完整域/特殊值检查，不能从显示文本剥偏移后走现有 1114 路径；当前无独立 1184 codec，SQLSTATE 与客户端往返未验证。 |

显式 timestamp→timestamptz 必须给定会话 TimeZone 或明确指定的区域，按该区日历与夏令时规则解释墙钟；反向先按目标区域显示再去时区，保留墙钟字段，会丢失瞬时唯一性（重复小时可映射相同 timestamp）。UTC 下 timestamp `2000-01-01 00:00:00` 转换为 epoch，Shanghai 下同一墙钟变为前日 UTC 16:00。NULL 保留目标类型，非有限值按 PG 特殊值规则传播；转换溢出 22008。显式 text↔timestamptz 依上表；date 相关转换、隐式/赋值资格及 AT TIME ZONE 语法/运算交本章转换块与工作流 2/4，不因共享 i64 自动允许隐式 cast。

MySQL TIMESTAMP 的既有时区转换需要原类型、存取阶段、来源时区与 Fsp 证据；显示字符串不足以恢复瞬时。DATETIME 无瞬时身份，必须显式指定解释区域；缺信息桥接拒绝 0A000，零/非法日期拒绝 22008。反向桥接校验 MySQL 目标范围/Fsp 与会话转换，BC、扩展年份及非有限值不能保真时 0A000；不裁剪、补零或改成 NULL，不改变原 Go/MySQL 宽松解析、错误和告警。内部表示需补瞬时身份、完整微秒域/特殊值、p 和 typed NULL 保真，不能只增加 OID 表。TimeZone 状态存储、SET/事务生命周期及读入时机由工作流 5 提供确定上下文；本块不实现该状态。

#### 时间间隔契约

PG interval 是独立规范身份，目标保留 `(months:i32, days:i32, time_microseconds:i64)`，允许分量独立负号；年归月、小时归微秒，不能把整数月折为 30 天、把整数日折为 24 小时或将 24 小时自动进位为日。身份和分量必须跨参数、cast、结果保真，不能用比较相等代替分量相同。[PG 18 §8.5.4–8.5.5](https://www.postgresql.org/docs/18/datatype-datetime.html#DATATYPE-INTERVAL-INPUT) 支持单位文本、SQL/ISO 8601 输入及四种 IntervalStyle 输出；固定 postgres 基线，并分别核对 postgres_verbose、sql_standard、iso_8601。输入 `1.5 months` 的小数部分按官方规则归到日，不能由此推广到整数月一律折日；IntervalStyle 影响未独立标号的负字段解释，会话配置交工作流 5。

精确布局以官方 **REL_18_0** 的 [datatype/timestamp.h](https://github.com/postgres/postgres/blob/REL_18_0/src/include/datatype/timestamp.h) 为基线：三个有符号分量，时间单位为微秒；不是 MySQL ±838 小时域。整数分量边界分别是 i32/i64 范围，三者全最小/全最大组合保留为 -infinity/infinity，其他组合不能误判非有限。文本解析、单位累加和精度收敛都须检查溢出；不能把表 8.9 的约 ±178000000 年当每种分量的统一上限。合法 PG 值若当前承载不足，显式 0A000 并登记缺口，不能截断或伪装 NULL。

OID=1186、typlen=16 由 [REL_18_0 pg_type.dat](https://github.com/postgres/postgres/blob/REL_18_0/src/include/catalog/pg_type.dat) 定义；typmod 未声明=-1，有声明按 [utils/timestamp.h](https://github.com/postgres/postgres/blob/REL_18_0/src/include/utils/timestamp.h) 的 `(range & 0x7fff)<<16 | (p & 0xffff)`，不是裸 p。fields 接受 YEAR/MONTH/DAY/HOUR/MINUTE/SECOND 及官方组合，精度 p=0–6；fields 未指定为 full range，fields-only 保留 full precision。按 [REL_18_0 timestamp.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/timestamp.c) 的 intervaltypmodin/AdjustIntervalForTypmod 截去最小允许字段右侧的部分，保留更高字段；p 舍入微秒时间，半值远离零，不能以截断替代。负 p 错误 22023，p>6 告警 22023 后归 6。SQL 声明语法交工作流 2。

| 方向 | 类型、值域/精度与 NULL | 编码、元数据、错误与差距 |
| --- | --- | --- |
| text 参数→规范值 | 明确 1186/目标 fields/p 后解析分量，按目标修饰符收敛；typed NULL 保留身份，不解析负一长度载荷。 | text 编码 UTF-8，无 NUL；非法语法 22007，区间解析溢出 22015，最终分量/typmod 舍入溢出按官方阶段 22008；非法编码/NUL 22021。现有 parameter 仅统一不支持，不证明这些分类。 |
| binary 参数→规范值 | 非 NULL 恰 16 字节；网络序依次 i64 微秒、i32 日、i32 月，符号独立；应用目标 typmod，NULL 与零值分开。 | interval_recv/send 为布局依据；短载荷 08P01、多余字节 22P03，帧长度错误 08P01，特殊组合识别一致。当前 binary_parameter 无 1186。 |
| 规范值→text 结果 | 根据 IntervalStyle 输出可回读文本，保留月/日维度、独立符号与微秒；NULL 长度 -1，不是空串或 0。 | Describe/Execute 恒为 1186/16/声明 typmod；输出格式不改类型，特殊值不是 NULL。现有 pg_type 无通用 interval 身份。 |
| 规范值→binary 结果 | 同一三分量直接按 16 字节布局输出，不经秒数总量或显示文本；NULL 长度 -1，零间隔为 16 字节全零。 | 同一 OID/typlen/typmod，格式码 1；当前出口未实现。内部非法状态不得静默绕过验证；错误恢复交工作流 7。 |

转换边界：显式 text↔interval 复用上述词法与输出契约；interval→同身份带 fields/p 的转换按声明收敛且传播 typed NULL。显式/赋值/隐式资格交本章转换块，不由 OID 推导任意数字、time、timestamp 可隐式互转。MySQL duration TIME 若显式桥接，仅在约定且可保真的 `(0,0,微秒)` 子域转换；反向要求月/日为零、时间精度与 MySQL 范围可承载，否则明确 0A000，不自动折算月/日。PG 合法性错误与暂缺桥接能力必须分开。日期加 interval、DST 与比较/justify 的运算实现归工作流 4；本块三分量不规定运算比较算法。以上规范来自 PG 18 文档及 REL_18_0 源码，不构成指定 18.6 的实测结论。

#### 通用唯一标识契约

UUID 目标为独立规范类型身份与固定 16 字节值，覆盖全部 128 位组合，包括全零、全 ff 和任意 version/variant 位；不因生成器只产生某版本而缩小输入域。没有数值精度、scale、时区或舍入参数，typmod=-1。依据 [PG 18 §8.12 UUID Type](https://www.postgresql.org/docs/18/datatype-uuid.html) 及固定版本 [REL_18_0 uuid.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/uuid.c)::string_to_uuid/uuid_out/uuid_recv/uuid_send；以下为目标规范，未实测指定 18.6。

文本接受大小写十六进制、可选的一对外层花括号、无连字符以及每四个十六进制字符组后可选连字符；必须恰好 32 个十六进制字符，连字符不得在末尾。标准例 `a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11`、大写、`{a0eebc999c0b4ef8bb6d6bb9bd380a11}`、`a0ee-bc99-9c0b-4ef8-bb6d-6bb9-bd38-0a11` 均归同一值。输出固定小写 8-4-4-4-12，去花括号；不保留输入拼写。空串、前后空白、URN 前缀、非法字符、奇数位、缺/多位、错位连字符及不配对花括号按 22P02 拒绝，不修剪、补位或截断。编码错误沿文本块的 22021 先行检查。

| 方向 | 身份、值域、NULL 与可观察目标 | 元数据、错误与现状差距 |
| --- | --- | --- |
| text 参数→规范 UUID | 已定型 2950 才按上述词法解码；NULL 保持 UUID 身份，无载荷，不等于全零值。 | ParameterDescription=2950；非法文本22P02。当前非NULL无2950分支，能力未接通时明确0A000。 |
| binary 参数→规范 UUID | 恰16字节，按规范文本从左到右每两位对应一字节；`00112233-4455-6677-8899-aabbccddeeff` 对应 `00 11 22 33 44 55 66 77 88 99 aa bb cc dd ee ff`。不采用主机整数序或GUID混合端序，不加version校验。 | OID2950；短载荷08P01，多余载荷22P03，不能静默消费前16字节。当前binary_parameter缺分支。 |
| 规范 UUID→text 结果 | 固定36个ASCII字节规范输出；NULL长度-1，非NULL全零仍输出完整UUID。 | RowDescription=2950/16/-1；文本格式码0，不得因显示字符串变成OID25。现有pg_type无法输出该身份。 |
| 规范 UUID→binary 结果 | 原样输出16字节；NULL长度-1，不能发送空载荷或ASCII十六进制。 | RowDescription=2950/16/-1；格式码1，只变格式不改身份。现有value_binary无2950分支。 |

OID/typlen 依据 [REL_18_0 pg_type.dat](https://github.com/postgres/postgres/blob/REL_18_0/src/include/catalog/pg_type.dat) 的 uuid 项。NULL、格式码和元数据字段依 [PG 18 §54.7 Bind/DataRow/ParameterDescription/RowDescription](https://www.postgresql.org/docs/18/protocol-message-formats.html)。短载荷与剩余字节诊断分别依据 [pqformat.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/libpq/pqformat.c)::pq_getmsgbytes 和 [postgres.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/tcop/postgres.c)::exec_bind_message；消息帧破损与Sync恢复交工作流7，不混同合法UUID值域错误。

显式 text→uuid 使用上述输入规则，uuid→text 使用规范输出，两方向传播typed NULL；目标结果身份随cast目标变化。普通text/bytea、MySQL CHAR(36)/BINARY(16)不因长度或外观自动成为UUID；bytea不自动获得PG UUID cast资格，尚无有证据桥接时明确0A000。显式、赋值与隐式资格由本章转换块统一核定，禁止无条件复用MySQL字符串强制转换。声明列、显式UUID类型常量/转换以及已声明2950参数应保持身份；OID0参数只能在解析上下文真正定型后使用，不能扫描首个值猜类型。当前通用Parse缺省OID拒绝规则仍为现状；语法交工作流2，推导和PreparedMetadata连续性由本章接线，不能仅加codec声称完成。

#### JSON文本契约

PG `json` 是独立规范身份，保存通过校验的原始JSON文本；不是普通text、MySQL BinaryJSON或PG jsonb的别名。OID=114、typlen=-1、typmod=-1；ParameterDescription与statement/portal Describe、Execute保持身份，不能由首行文本外观推定。SQL声明、明确cast及参数上下文负责定型，OID0推导沿本章公共契约；精度/scale、时区不适用于文本载体，不添加numeric修饰符。

输入允许对象、数组、数字、字符串、true/false/null作为完整根值，保留前后合法JSON空白、键顺序、重复键全部成员、转义拼写、数字尾零和指数。数字只按JSON词法校验，不先转浮点或numeric：`9007199254740993`、`18446744073709551616`、`1e1000000`原样保存，不承诺算术可用或无限资源。`01`、`+1`、`NaN`、`Infinity`、大写TRUE、注释、尾逗号及多根值非法；空串/纯空白不是JSON null。资源上限须独立取证，不能以静默截断或有损降级成功。

固定UTF-8数据库及客户端编码时，直接Unicode字符必须编码合法；JSON字符串内合法四位十六进制转义保持词法，`"\u0000"`及孤立代理转义`"\uD800"`在原始输入中允许。这不承诺函数解码或jsonb转换成功，解码错误另按处理阶段验收；原始NUL字节不是转义序列，传输编码失败目标22021。非法JSON词法/结构目标22P02，两种格式同样校验，binary不因名称而免检。

| 方向 | 类型、值/NULL、错误与wire目标 | 当前差距 |
| --- | --- | --- |
| PG text参数→规范json | OID114定型，编码转换后校验完整文档并保留原文；SQL NULL长度-1单独标记，四字节`null`为非NULL。非法JSON22P02、非法编码22021。 | parameter无114分支；NULL提前返回不证明身份连续。 |
| PG binary参数→规范json | 同为OID114，载荷是JSON文本字节，无版本前缀、无MySQL类型码；按连接/服务器编码转换和校验，再保留文本。NULL=-1；零长度是非法文档22P02。帧长度不一致08P01交工作流7。 | binary_parameter无114分支；不能借bytea或jsonb codec。 |
| 规范json→PG text结果 | OID114/-1/-1，直接输出所存文本并按客户端编码转换；UTF-8到UTF-8逐字节保留。NULL=-1，JSON null长度4；失败不得回空串成功。 | MarshalJSON/String重新序列化，pg_type拒绝245，不具备原文出口。 |
| 规范json→PG binary结果 | 相同元数据，载荷仍为编码后的原文文本，无jsonb版本字节；NULL及错误约束同text。格式码1不代表内部BinaryJSON内存布局。 | 当前无114结果codec或原文身份。 |
| text↔json显式转换及MySQL桥接 | text→json须校验且保留文本，json→text返回存量原文；NULL保持SQL NULL。不将外观相同text隐式变json；隐式/赋值资格交统一转换块。MySQL BinaryJSON已丢失的重复成员/词法不能恢复为原文，只有明确且可验证的转换契约才能产生新的json表示，否则项目拒绝0A000。 | 共用JSON解析器或函数输出不证明PG cast资格；MySQL原Go行为独立保留，不作PG有损回退。 |

官方依据（2026-10-23查阅）：PostgreSQL **18** §8.14 [JSON Types](https://www.postgresql.org/docs/18/datatype-json.html) 定义原文、重复键、Unicode输入与json/jsonb数字域差异；精确codec核对 **REL_18_0** [json.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/json.c) 的 json_in/json_out/json_recv/json_send，输入校验后存text、两格式发送文本；[pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat) 确定114及变长。§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 支持格式、长度/NULL及描述字段，[附录A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 支持22P02/22021/08P01/0A000分类；精确诊断与阶段仍需参考端采集。来源18.6目标保持不变，REL_18_0静态源码不是18.6实测。

#### JSONB规范值契约

PG `jsonb` 保留独立规范身份，OID3802、typlen=-1、typmod=-1；与OID114原文json、text、MySQL245分别定型。保存分解后的对象、数组、字符串、精确number、boolean和JSON null；不承诺原始空白、对象输入键序、重复键或数字原词法。嵌套各层重复键仅保留最后值，键按解码后的字符串识别；数组顺序和重复元素保留，标量不能强制包成数组。SQL NULL是外部缺值，JSON null是非NULL值；精度修饰符、时区、SQL数组维度/下界不适用此标量族。

number沿PG numeric有限数值域精确保留，禁止借f64或受限MySQL decimal静默舍入；超出PG域应拒绝，PG域内暂不能保真则0A000，不能伪称成功。输出按numeric展开指数并保留小数尾零，例如`1.230e-5`输出`0.00001230`；相同数值的不同scale不代表必须输出同字节。NaN/Infinity不是合法JSON number。固定UTF-8时，Unicode转义解码，正确代理对合并；拒绝`\u0000`、孤立/错误代理对与非法编码。对象键顺序不是用户输入顺序；REL_18_0按UTF-8键字节长度再按字节排序，不能用BTreeMap字典序替代。对参考端输出的字节断言须绑定精确构建，不能将内部排序当成跨版本存储格式。

| 方向 | 目标类型、值/NULL及wire契约 | 现状与拒绝边界 |
| --- | --- | --- |
| PG text参数→规范jsonb | OID3802按连接编码接收JSON文本，完整校验、精确数字解析及规范化；-1长度为SQL NULL，`null`为JSON null，空文档非法。 | parameter缺3802分支；未补齐身份与保真前0A000。 |
| PG binary参数→规范jsonb | OID3802载荷为单字节版本01加JSON文本，无内部容器头；版本后仍做同等语义校验。SQL NULL=-1无版本字节。 | binary_parameter缺3802分支；禁止把MySQL BinaryJSON、PG磁盘布局或无版本json文本直接接受。 |
| 规范jsonb→PG text结果 | 输出PG规范JSON文本；RowDescription保持3802/-1/-1，格式0；空集、首行NULL和全NULL均不改变身份。 | pg_type无通用JSON出口；禁止回落text OID25。 |
| 规范jsonb→PG binary结果 | 同一元数据、格式1；载荷01加与text输出对应的JSON文本，长度含版本字节，SQL NULL=-1无载荷。 | 没有通用3802编码；不是直接输出内存字节或json的无前缀binary。 |
| 显式cast与MySQL桥接 | text/json→jsonb重新校验、规范化；jsonb→text/json输出规范文本，无法恢复原始重复键、空白或键序；NULL保持SQL NULL。隐式转换资格由方言先决策，不能自动接受普通text。 | MySQL扩展类型及不能精确保真的数值必须显式拒绝0A000；不重解释已有Go/MySQL函数；cast资格交后续转换块。 |

非法JSON目标22P02，数值越PG numeric域目标22003，无法表示的Unicode转义目标22P05，非法UTF-8/原始NUL按编码阶段22021；错误必须记录阶段。binary空载荷缺版本及外层帧截断按协议错误08P01验收；仅版本01而没有JSON是22P02，版本00/02/ff必须拒绝。未知版本在REL_18_0的jsonb_recv使用无显式errcode的elog(ERROR)，不能擅自统一成22P03；其诊断/SQLSTATE以精确参考构建采集为准。合法版本后尾随垃圾也必须拒绝，不能截取首个值。协议帧长度与恢复归工作流7；尚未支持的codec沿0A000拒绝，不把该临时错误当作最终PG非法输入诊断。

官方依据（2026-10-23查阅）：PostgreSQL **18** §8.14/§8.14.1 [JSON Types](https://www.postgresql.org/docs/18/datatype-json.html) 支持重复键、数字、Unicode与输入输出目标；精确wire核对 **REL_18_0** [jsonb.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/jsonb.c)::jsonb_in/jsonb_recv/jsonb_out/jsonb_send，明确版本1加文本；[jsonb_util.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/jsonb_util.c)::lengthCompareJsonbString/lengthCompareJsonbPair/uniqueifyJsonbObject 支持键排序及最后值；[pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)确定3802及变长。§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 支持格式、长度/NULL与描述字段；[pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c)::pq_getmsgint 支持缺版本读取的协议错误边界，[附录A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持SQLSTATE分类。以上为静态规范依据，18.6仍是来源目标，未核实该版本实际运行行为。

#### 基础数组契约

基础数组是带元素类型身份的矩形多维值，保存维数、各轴长度和有符号下界、按最后一维变化最快排序的元素及逐元素NULL标记；数组本身SQL NULL另存。`integer[]`与`integer[][]`同属int4数组类型，维数属于值，不形成不同OID，声明大小不充当运行时约束。基础范围采用前述内建标量族，逐族只有身份、输入输出与保真已获证后才开放其数组；不能因标量存在即承诺整个数组族。int4/数组OID为23/1007，text为25/1009，bool为16/1000，bytea为17/1001，int2为21/1005，int8为20/1016，json为114/199；其他族开放前核对官方目录，不能按OID加减推算。RowDescription使用数组OID、typlen=-1；本块无修饰例typmod=-1，有修饰元素按该族typmod传递，不编码维数。内部头的OID是**元素OID**，不能填数组OID。

默认下界1；`[0:1][-2:-1]={{1,2},{3,4}}`保存shape=[2,2]、lower=[0,-2]，不能重排或改为从1起。空数组规范化为零维、零元素，text为`{}`、binary头ndim=0；不要求保留输入零长度轴的虚构下界。它仍有元素类型且不等于数组NULL或含NULL的非空数组。NULL数组无形状载荷；`{NULL}`是一元素非NULL数组。同一轴长度须一致，不能补NULL修复参差形状。REL_18_0最大维数6；维数、维长乘积、边界加法和分配大小须检查溢出，不能截断。有类型空数组不从首元素推断，裸空构造的定型交第2章和转换块。精度、舍入、时区和特殊值沿元素族契约，数组容器不额外降低精度；容器本身无数值精度/时区。

| 方向 | 类型、值、NULL与元数据目标 | 现状差距与错误责任 |
| --- | --- | --- |
| PG text参数→规范数组 | 声明数组OID确定元素输入规则及typmod；花括号、可选`[l:u]...=`前缀共同确定矩形形状；外层长度-1是数组NULL。 | 普通Parse白名单无数组；不能绕为text/JSON成功。未补齐保真前0A000；输入词法/形状与元素错误分层报告。 |
| PG binary参数→规范数组 | 按下述网络字节序头、维度和元素载荷读取；外层-1无头，内层-1仅对应元素NULL。数组OID决定期望元素OID。 | binary_parameter缺通用分支；目录接线不可当通用参数入口。非法头、类型与长度按下述分类拒绝。 |
| 规范数组→PG text结果 | 由规范形状生成花括号和必要下界前缀，逐元素调用该族text输出再数组转义；格式0，稳定数组OID/-1/typmod。 | 现有value_text透传目录字符串不足；空集、首行NULL、全NULL均不得降为text25。 |
| 规范数组→PG binary结果 | 同一规范值按维度/下界输出头并逐元素使用元素binary编码；格式1，元数据与text一致，数组NULL外层-1。 | binary_array仅目录一维/固定下界；不能先扁平为目录字符串再丢失形状、下界、元素NULL。 |
| cast/推导与MySQL桥接 | 数组共同元素类型及隐式/显式资格先按PG决策；允许的逐元素cast保留形状、下界和NULL，失败不返回部分转换结果；元素精度/溢出沿前述类型族。 | text显式输入按目标数组解析；JSON数组、MySQL typed array/vector、目录vector不自动等价。缺保真明确0A000，Go/MySQL原行为不变；完整cast资格由后续转换块细化。 |

文本codec固定UTF-8、`array_nulls=on`作为基本验收前提（其他会话配置先明示支持或拒绝，状态归工作流5）。未引用且未转义的NULL不区分大小写，表示元素NULL；`"NULL"`、转义形成的NULL字样是字符串，`""`是空字符串。含分隔符、花括号、双引号、反斜线、空白或NULL字样的元素输出须双引号包围，内部双引号/反斜线用反斜线转义；逐元素按类型的typdelim处理，基础例用逗号，不把所有PG类型都假定为逗号。SQL字符串字面量的词法转义与数组文本转义是两层：Bind载荷不再做SQL反转义。引号不闭合、残缺转义、缺元素、尾逗号、参差子数组及维度前缀不匹配拒绝22P02；元素非法文本/溢出分别沿元素族（如int4非法22P02、超域22003），非法UTF-8按编码阶段22021。完整ARRAY构造语法归工作流2，其非矩形错误阶段不能直接套用文本输入诊断。

二进制codec使用网络字节序：Int32 ndim、Int32 flags（0或1）、UInt32 element_oid；随后每维Int32 length及Int32 lower；再按行主序为每个元素输出Int32 byte_length及该元素binary载荷。内层length=-1为NULL、0可为合法空变长元素，无元素分隔符、内存对齐填充或PG磁盘varlena头。输出flags反映是否含NULL；输入flags仅允许0/1，不能臆加“flags=0必须无NULL”的参考端拒绝条件。非空int4数组`{1,NULL}`的字段序列为`1,1,23; 2,1; 4,00000001; -1`；空int4数组为`0,0,23`，仍非零字节载荷。

负ndim/非法flags目标22P03，超过6维54000；内建元素OID与期望不符42804；非法维长、维度乘积/下界溢出按参考端限制错误验收（REL_18_0 ArrayGetNItems/ArrayCheckBounds 的精确分支需采集）。元素长度<-1、声明元素长度大于剩余载荷或元素未消费完整目标22P03；不足以读取Int32头/长度的截断与外层Bind/DataRow帧不完整区分，协议读失败目标08P01，元素内部错误沿该族，不能一概归22P03。尾随垃圾须拒绝，精确阶段/码按参考构建采集；禁止忽略超出维长乘积的元素。尚未开放的合法族/codec以0A000拒绝，该临时门禁不能替代最终非法输入诊断。错误响应和Sync恢复交工作流7。

官方依据（2026-10-23查阅）：PostgreSQL **18** §8.15.1/.2/.6 [Arrays](https://www.postgresql.org/docs/18/arrays.html) 支持类型/维数、形状、引用、NULL及下界规则；精确格式以 **REL_18_0** [arrayfuncs.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/arrayfuncs.c)::array_in/array_out/array_recv/ReadArrayBinary/array_send 固定空数组规范化、头字段、元素类型/长度和错误分支，[array.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/utils/array.h) 固定MAXDIM=6及行主序，[pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)固定上述元素/数组OID配对。§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html)支持外层格式、NULL长度与描述元数据，[附录A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持SQLSTATE名称。源码tag与大版本手册仅定义目标，18.6仍是来源指定基线，未在该补丁版本运行验证。

#### 显式隐式转换契约

本块承接前16个类型块及第2章转换语法，定义类型分析到值转换的交接。E为显式CAST/::；A为赋值（INSERT/UPDATE目标列）；I为表达式隐式coercion。资格满足I可用于I/A/E，A只用于A/E，E只用于E；同类型仍须执行目标typmod检查。存在路径不保证任意值成功，也不因两类型经text可达就自动串接转换。以下是基础族的有向矩阵，未列组合默认不开放，后续必须取证PG18的具体规则；合法但尚未实现的路径明确0A000，不能伪称参考PG拒绝。

| 源→目标与方向 | E / A / I资格 | 值域、精度、NULL与错误 | 目标身份、wire及现状差距 |
| --- | --- | --- | --- |
| int2→int4→int8→numeric→float4→float8及该链前向内建数值对；反向对 | 前向E/A/I；反向E/A，禁止I | 目标域检查；近似浮点可能损精度，不能称无损提升；numeric/float转整数按各族舍入后查界；越界22003。 | 分别21/23/20/1700/700/701；typmod由结果表达式决定，不继承源修饰符；现有MySQL数值求值仅候选。 |
| int4↔bool | 两向仅E | int4零→false、非零→true；bool→0/1；不推广到所有数值类型或条件真值。 | 23↔16；NULL仍是目标typed NULL；当前MySQL整型载体不证明bool身份。 |
| text/varchar/bpchar内部互转及同类型长度约束 | 基础互转E/A/I；长度行为须区分上下文 | 显式超长截断与赋值超长22001沿文本块，bpchar补空格/去尾空格保留；不能用UTF-8字节截断代替字符规则。 | 25/1043/1042及目标typmod；二进制可coerce不等于长度约束免检。现有cast字符串规则未证PG等价。 |
| 已定型text→bytea/整数/numeric/float/bool/date/time/timestamp/timestamptz/interval/UUID/json/jsonb/基础数组 | 输入输出通用规则允许E；禁止由此开放A/I | 调用目标输入语义，非法文本22P02或时间族22007/22008，超域按族；不得只取数值前缀。 | 目标OID/typmod沿各族；数组输入另保留shape/lower。UUID/jsonb/数组仍有表示与codec缺口。 |
| 上述基础标量/数组→text/varchar/bpchar | 通用输出转字符串允许E/A；不因此开放I；具体目录条目优先 | 使用源类型输出规则，再施加目标字符约束；bytea按自身输出格式，不能把原字节当UTF-8。 | 25/1043/1042；非字符串原身份不能因值像字符串改变。当前结果文本输出不能自动当SQL cast。 |
| date→timestamp、date→timestamptz、timestamp→timestamptz；逆向timestamp/timestamptz→date、timestamptz→timestamp | 前三条E/A/I；列出的逆向E/A | date补午夜；瞬时与墙钟转换使用会话TimeZone，丢时间/区域信息不承诺互逆；范围/特殊值沿时间块。 | 1082/1114/1184；目标p与typed NULL保留。MySQL零日期、隐含来源区域不能直接复用。 |
| timestamp/timestamptz→time；time→interval；interval→time | 前者E/A；time→interval为E/A/I；逆向E/A | 提取日内时间，interval回time的日内归约按PG参考取证，不拿MySQL duration直接映射；精度沿族。 | 1083/1186；月份/天不能错误合并为总秒；现有Duration不是PG interval。 |
| json↔jsonb | 两向E/A，禁止I | json→jsonb校验、规范化且可能失去原文；逆向只得规范文本；jsonb更严格数值/Unicode校验沿族。 | 114↔3802，SQL NULL不同于JSON null；现有BinaryJSON不是任一方向的充分证据。 |
| T[]→U[]（本章已列基础族） | 由元素T→U在相同上下文的资格决定，不因容器可表示放宽 | 逐元素检查，保留维数、维长、下界及元素NULL；单元素失败拒绝整个值，无部分结果。 | 目标数组OID及其元素OID一起改变，typmod沿元素约束；现有JSON typed array不是PG数组。 |
| bytea↔其他非字符串族、UUID↔bytea、任意未列跨族/MySQL桥接 | 本块不新增资格；不能由相同字节宽度或目录OID推导允许 | PostgreSQL18已有局部路径（如整数↔bytea）须单独取证后开放；MySQL unsigned可证提升到numeric，零日期/负或长TIME无等价路径则拒绝0A000。 | 禁止只换OID；缺合法路径时类型分析拒绝，缺实现时能力门禁。精度/时区不适用的固定字节族不人为添加舍入。 |

矩阵各行共同NULL契约：先检查类型资格，再对合法严格转换传播SQL NULL；已定型的非法类型对不能因值为NULL绕过资格检查。unknown NULL可按上下文定型，typed NULL保持目标OID/typmod，不执行值解析；数组NULL、元素NULL、JSON null分别沿各族。本矩阵不是PG全量cast目录；pg_cast之外还有I/O、数组等通用规则，不能以目录无条目认定不存在。

unknown字面量、未定型NULL、Parse未指定OID参数是不同来源，必须保留来源信息。字符串字面量初始unknown，`'1'::int4`直接调用int4输入；`'1'::text`已定型，不能继续当unknown隐式转数值。OID0只表示客户端未指定，Parse分析可由`$1::int4`或目标列确定23，不能读取Bind值猜测。重复$n是同一个参数身份；首次上下文推定类型后，后续引用检查一致性或插入合法coercion，禁止每次引用各自解码。显式OID先固定源类型，再检查上下文转换。无法确定类型目标42P18，不一致推断42P08，具体触发优先级/错误位置须参考端采集；与普通cast失败42846、共同类型/赋值不匹配42804及运算重载失败42883分责。

共同类型推导适用于UNION/INTERSECT/EXCEPT逐列、CASE结果、VALUES列及ARRAY等构造；不能复用MySQL in_union开关代替规则。相同非unknown身份优先；全部unknown定为text，否则忽略unknown选择候选，不同类别拒绝；同类别从左向右依据单向隐式可转换性更新候选，优选类型可终止搜索，最后所有输入均须可隐式转候选。CASE历史顺序以ELSE作为首候选；嵌套UNION逐对解析，不能把全树一次提升。domain不属于本轮开放范围，遇到时能力门禁，不能冒充基础类型支持。运算符/函数重载的候选选择归工作流4，接收本块资格结果而非套用集合算法。

方向责任：parser（工作流2）保留cast目标、参数编号及原位置；类型分析拥有E/A/I资格、共同类型及参数约束；常量折叠可在准备期触发非法值，不能保证所有转换错误延后Execute。Bind codec只按确定参数身份解码，格式0/1不改变SQL类型；求值转换负责目标值域、舍入后溢出与时区；结果codec按固定结果身份编码，不再推导cast。ParameterDescription返回推定参数OID，RowDescription返回分析结果OID/typmod；零行、全NULL、换Bind值以及两种输出格式均不得改变描述，详细生命周期交批次18。PG错误必须拒绝语句，不沿MySQL警告后继续返回裁剪值；MySQL原Go语义保持独立。错误响应/Sync恢复归工作流7；P若提供按原SQL字符位置，精确值及错误阶段均待差分。

方案比较：直接复用共享转换器可减少实现，但现有截断、unsigned、decimal钳制和MySQL日期路径已有反例，不能证明PG资格与值语义等价。选择“连接方言与E/A/I先决策，再复用已证实等价的局部转换”；可候选复用NULL传播、合法定宽整数值操作等，但须把类型检查、PG范围错误与元数据连续性放在调用边界，逐项差分证明后采用。只补当前类型族必需信息，不重建类型系统，不改既有Go行为。

官方依据（2026-10-23查阅）：PostgreSQL **18** [§10.1](https://www.postgresql.org/docs/18/typeconv-overview.html)支持unknown与上下文分类，[§10.4](https://www.postgresql.org/docs/18/typeconv-query.html)支持赋值和typmod，[§10.5](https://www.postgresql.org/docs/18/typeconv-union-case.html)支持共同类型顺序，[PREPARE](https://www.postgresql.org/docs/18/sql-prepare.html)支持参数上下文推定；[§52.10 pg_cast](https://www.postgresql.org/docs/18/catalog-pg-cast.html)定义E/A/I层级和目录非穷举性。具体方向以 **REL_18_0** [pg_cast.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_cast.dat)的numeric、bool、string、datetime、json条目为静态依据；通用I/O与数组资格已核对[parse_coerce.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/parser/parse_coerce.c)::find_coercion_pathway的字符串I/O与元素递归分支。[§8.1](https://www.postgresql.org/docs/18/datatype-numeric.html)支持数值范围/舍入，[附录A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持错误分类。18.6是来源指定参考目标，本轮未运行该版本或本地SQL；以上均为文档目标，非功能通过声明。

#### 准备描述执行元数据契约

本块承接各类型族及[显式隐式转换契约](#显式隐式转换契约)，固定规范身份→参数/结果描述→codec的责任。Prepare在此指共享准备入口及分析阶段；SQL PREPARE不是wire Parse消息，不能据名称相同推定两种入口均可用。声明类型、参数约束和表达式分析决定类型，不用首行、NULL、值大小或MySQL列名猜测。值域、精度、舍入、时区和数组形状继承对应族；本块不新增值域。typlen是类型固定长度或负的变长标记，不是当前载荷字节数；typmod是类型专用修饰符，不是OID或格式码。

| 方向/阶段 | 类型、NULL、值域与wire目标 | 现状差距及拒绝责任 |
| --- | --- | --- |
| Parse/Prepare→定型 | 显式非0参数OID作为输入约束；省略或0交分析按上下文推断。相同$n的所有出现约束须一致，参数按编号描述，不能按出现次数重复。结果列数/顺序/名称、OID、typlen、typmod由分析结果确定。无值阶段NULL载荷不适用，typed NULL表达式仍有身份。 | 普通路径目前要求显式OID且桥接只暴露参数数量。目标不能推断时拒绝42P18，约束冲突按实际语义诊断；合法但尚未开放的推断明确0A000，不伪称参考端也拒绝。不得执行原查询或样本查询取得类型。 |
| statement Describe→客户端 | ParameterDescription仅含参数数量及OID，无参数typlen/typmod/格式字段；随后RowDescription含结果列数及每列身份/typlen/typmod，格式恒0。无返回行的命令给NoData；返回零行的SELECT仍有描述。值/载荷/值域检查不适用，因为没有绑定值。 | 当前description复用零行QueryResult编码；固定-1不证明声明精度保真。缺类型须显式拒绝，不能伪造text或NoData。 |
| Bind→portal | 参数数量匹配定型参数；输入格式0/1独立于结果格式。NULL长度-1无载荷，按已定型身份保留，不改OID；非NULL按该族值域及codec校验。参数格式数量0/1/N，结果格式数量0/1/列数，0项默认text，1项广播，否则逐项。绑定值只改变值或计划选择。 | tp=6提前返回不能证明typed NULL跨层保真；无支持的binary codec明确拒绝0A000，非法text/binary分别按族22P02/22P03等，数量/格式码错误08P01；不退回text。 |
| portal Describe→客户端 | 只返回RowDescription或NoData，不返回ParameterDescription；身份/列数与statement一致，格式按该portal Bind选择，可逐列混合。typlen/typmod不随格式改变；NULL和值域不参与描述推断。 | Portal已有独立快照；须证明两个portal格式互不污染，不能把statement的0格式原样当binary描述。 |
| Execute→结果codec | 按portal固定类型/列数编码各行，NULL两格式均-1，零行仍保持已知描述。扩展Execute本身不再发送RowDescription；对照portal Describe与实际载荷，不要求不存在的Execute描述包。重复Execute消费同一portal进度；新Bind才提供另一组值。 | 当前编码后比对描述提供局部保护，仍须证明缺类型、行宽、OID、typlen、typmod变化在发送任何DataRow前拒绝。无参数回填只能使用可信准备身份，不能遮蔽真实类型变化；参数化投影必须有明确身份连续性。 |

合法换Bind值、首行/全NULL、零行、text/binary、值相关计划选择均不得改变类型身份。DDL或解析环境变化可能触发重新分析；固定结果的准备语句若结果类型/typmod/列数变化应拒绝0A000，不能静默替换已描述形状；重规划本身不保证出错，也不能承诺永远不重绑定名称。对象已不存在等情况按实际诊断，不统一包装成元数据变化。缺规范类型属于AsterSQL能力拒绝0A000，不是PG对合法SQL的预期错误；具体报错阶段须参考端采集。工作流7拥有消息顺序、错误传递、Sync恢复与portal生命周期，工作流5/6拥有事务/目录失效；本块只固定类型连续性及拒绝边界。MySQL Prepare及原Go行为独立保留，不能把PG OID直接当MySQL类型标志。

官方依据（2026-10-23查阅）：PostgreSQL **18** §54.2.3 [Extended Query](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-EXT-QUERY) 支持Parse/Bind/两种Describe与Execute的分责；§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 的Parse、Bind、ParameterDescription、RowDescription、DataRow支持上述字段、格式数量及NULL长度。推断失败以 **REL_18_0** [postgres.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/tcop/postgres.c)::pg_analyze_and_rewrite_varparams为依据；固定结果变化拒绝以同tag [plancache.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/cache/plancache.c)::RevalidateCachedQuery为依据。手册与源码定义目标，18.6仍是来源指定基线，没有该补丁版本或本地协议实测。

#### 章节验收与后续交接

本块只收束文档。以下索引与各族方向表、验收条目共同组成完整矩阵，不以索引行代替其精确值域、精度、SQLSTATE或二进制格式。来源18.6仍是指定目标，未取得该精确构建的运行证据；所有能力当前为**未验证**，源码与独立测试只作静态入口。

| 来源步骤 | 累计覆盖位置 | 收束条件 |
| --- | --- | --- |
| 1：双向映射、推导、NULL | 规范类型与协议映射；准备描述执行元数据契约 | 参数类型、规范身份、结果描述及MySQL标志分别核对，TYPE-MAP与META互相引用；OID映射不是支持证明。 |
| 2：全部类型族 | 布尔至基础数组的15个类型族块（整数含三个宽度，文本含三种身份） | 逐族按下表展开四方向、边界及拒绝目标，目录专用类型不能替代通用类型。 |
| 3：cast及不等价边界 | 显式隐式转换契约及各族桥接段 | CAST条目覆盖E/A/I、精度溢出、时区、数组维度/下界；unsigned、零日期、duration保留双方言差异。 |
| 4：验收矩阵及生命周期 | 各族验收表、TYPE-MAP、META | 四方向与NULL/边界/非法输入均有目标、错误和描述断言；消息状态与恢复交工作流7。 |

为每个下表类型建立能力ID `TYPE-ACCEPT/<族>/<IT|IB|OT|OB>/<VALUE|NULL|EDGE|INVALID>/pg`，这是**待新增登记约定**，不是既有测试名或当前通过记录。IT/IB为text/binary参数输入，OT/OB为text/binary结果输出。每格继承对应族列出的身份、值域/精度与错误；四方向均须记录参数OID或结果OID/typlen/typmod、实际格式、预期/实际值和SQLSTATE，不能留白。输入NULL固定外层-1且保留已定型身份；输出NULL同为-1，另跑首行NULL、全NULL和零行META场景。EDGE同时验上下端/特殊值；INVALID在输入测非法文本、长度、越域，在输出用受控非法内部值/缺身份夹具（待新增）验证拒绝，不能通过构造非法SQL冒充输出检查。无成功结果的拒绝场景，结果载荷和成功列元数据写“不适用（拒绝且无DataRow）”，仍断言拒绝前未发送成功结果。成功SQLSTATE写“不适用（成功）”。

| 族与既有验收索引 | IT正例 / 反例 | IB目标 / 非法输入 | OT / OB目标与边界 | 当前静态入口与缺口 |
| --- | --- | --- | --- | --- |
| bool；BOOL | true→真 / unknown拒绝22P02 | 02→真 / 超长拒绝22P03 | t / 01；16/1/-1，false及NULL独立 | pg_types_test、pg_extended_test有局部断言；四方向全集未验证。 |
| int2/int4/int8；INT | 1 / 各宽度上限+1拒绝22003 | 网络序定宽 / 短载荷按族长度错误 | 十进制 / 2、4、8字节；21/23/20及对应typlen，typmod=-1；负端与unsigned提升 | pg_types_test、pg_extended_test；宽度/unsigned binary及typed NULL缺证，未验证。 |
| numeric；NUM | 12.340 / 12bad拒绝22P02 | base-10000格式 / 非法头或digit拒绝 | 精确保真文本 / numeric格式；1700/-1/族typmod；舍入、负scale、大精度及特殊值 | mydecimal_test、pg_types_test；共享Decimal域与PG目标不同，未验证。 |
| float4/float8；FLOAT | 1.5 / 域外数值拒绝22003 | 4/8字节位型 / 错长度拒绝 | 目标宽度可逆文本 / 网络位型；700/701、4/8、-1；±0、无穷、NaN | pg_types_test局部binary断言；窄化及全边界未验证。 |
| text/varchar/bpchar；TEXT | é / NUL或非法编码拒绝22021 | 编码字节 / 同一编码拒绝 | 编码文本 / 同一字节；25/1043/1042及声明typmod；空串、长度/尾空格与22001 | pg_types_test、pg_extended_test；声明身份与长度保真未验证。 |
| bytea；BYTEA | hex 00ff / 奇数hex拒绝22P02 | 原始00ff / 帧截断拒绝08P01 | 选定bytea_output / 原始字节；17/-1/-1；空字节非NULL | pg_extended_test；escape与设置组合未验证。 |
| date；DATE | 2024-02-29 / 2026-02-29拒绝22008 | 四字节日序 / 超日期域22008 | DateStyle文本 / 日序；1082/4/-1；BC、有限端点、±infinity，零日期拒绝 | pg_types_test、pg_extended_test；完整日历域及binary输入未验证。 |
| time；TIME | 24:00:00 / 负duration拒绝 | 微秒日内域 / 越域拒绝 | 日内文本 / 微秒；1083/8/精度typmod；p舍入及24时边界 | pg_extended_test当前反而拒绝24时；完整目标未验证。 |
| timestamp；TS | 含6位小数的墙钟 / 零日期拒绝 | 微秒墙钟 / 越域拒绝 | 无时区文本 / 微秒；1114/8/精度typmod；BC、特殊值、舍入 | pg_types_test、pg_extended_test；扩展域及精度连续性未验证。 |
| timestamptz；TZ | 两种偏移同一瞬间 / 非法区域拒绝 | 瞬时微秒 / 越域拒绝 | 会话区域显示 / 同一瞬时；1184/8/精度typmod；DST、特殊值 | 前序TZ模块静态入口；通用瞬时表示与四方向未验证。 |
| interval；IV | 1 month 1 day / 非法单位拒绝 | 微秒/日/月三分量 / 非法长度拒绝 | IntervalStyle文本 / 三分量；1186/16/fields与p typmod；混合符号、非有限及溢出 | 前序IV模块静态入口；不得折为总秒，四方向未验证。 |
| uuid；UUID | 合法128位文本 / 非hex拒绝22P02 | 16字节 / 错长度拒绝 | 规范小写文本 / 原字节；2950/16/-1；全0及全ff，无数值精度 | 前序UUID模块静态入口；专属身份与通用codec未验证。 |
| json；JSON | 保留重复键原文 / 非法语法拒绝22P02 | JSON文本字节 / 非法编码拒绝 | 原文 / 同一原文；114/-1/-1；JSON null与SQL NULL分开，大数原文保留 | 前序JSON模块静态入口；MySQL BinaryJSON不能证明原文保真，未验证。 |
| jsonb；JSONB | 重复键取最后值 / 非法Unicode拒绝 | 版本1加文本 / 错版本拒绝 | 规范语义文本 / 版本1加文本；3802/-1/-1；numeric域和scale | 前序JSONB模块静态入口；精确值与版本codec未验证。 |
| 基础数组；ARRAY | 非1下界矩形数组 / 参差拒绝22P02 | 维度/元素OID/逐元素载荷 / OID不符42804 | 带必要下界文本 / 维度头与元素codec；数组OID/-1/元素typmod；空、数组NULL、元素NULL分开 | 前序ARRAY模块及目录局部测试；通用shape/lower保真未验证。 |

表中未重复的精确错误分支、有限端点和格式字段全部按对应族正文及验收表逐格继承，不能统一套22P02或22P03；合法但未开放的表示/codec明确0A000，不能改写PG目标为拒绝。类型族矩阵需要IT→OT、IT→OB、IB→OT、IB→OB四组合均验证同一规范值。转换另按CAST矩阵验E/A/I与typed NULL；Prepare/Describe/Execute按META七组验全链。数组逐元素族展开，标量通过不自动开放数组。MySQL关联ID用/mysql，独立保留原Go契约，不要求两端OID相等。

本轮查阅官方PostgreSQL **18** [第8章](https://www.postgresql.org/docs/18/datatype.html)确认各族规范入口，[第10章](https://www.postgresql.org/docs/18/typeconv.html)确认转换上下文分责，[§54.7](https://www.postgresql.org/docs/18/protocol-message-formats.html)确认Bind、ParameterDescription、RowDescription、DataRow字段；精确值/二进制细节继续绑定前序块的具体小节及源码版本。本块不新增二进制规范断言，不把18手册当18.6实测。

| 工作流 | 输入/输出交接与验收责任 |
| --- | --- |
| 2：方言身份与解析边界 | 输入连接方言、类型名称、cast/数组AST、$n编号；本章返回类型与转换约束，语法/名称拒绝仍归2。 |
| 4：方言函数与运算符契约 | 接收类型、共同类型及cast资格；负责重载、运算、返回类型及算术错误，不由codec决定运算语义。 |
| 5：SQL会话与事务语义 | 提供TimeZone、DateStyle、IntervalStyle、bytea_output、编码及事务时间；负责状态/失效，本章验配置下的值与元数据。 |
| 6：系统目录与内省契约 | 提供类型/对象身份及修饰符、失效信息；目录特例不能证明通用族，目录查询形状归6。 |
| 7：线协议认证与数据通道 | 接收已定型身份、参数/结果codec与SQLSTATE目标；负责帧、消息顺序、portal、错误传递和Sync恢复。 |
| 8：客户端与双方言共存验收 | 接收以上能力ID及四方向/边界用例；锁定参考构建、客户端和配置，按第1章采集差分及MySQL独立回归证据。 |

### 范围外

章节验收与后续交接不运行数据库或测试、不建立差分平台，不修改工作流2/4/5/6/7/8正文，不以文档验收承诺完整支持；未开放类型与扩展生态保持原范围。

[规范类型与协议映射](#规范类型与协议映射)只定义映射与交接，不实现生产代码、类型系统、完整 cast、codec、数据库环境或差分平台。逐族输入语法、舍入/时区、数组形状及精确二进制布局交本章后续文档块，不因列出 OID 提前承诺支持。parser/名称绑定归工作流 2；运算重载归 4；时区等会话状态归 5；目录类型身份归 6；消息状态机、格式选择、错误响应和恢复归 7；真实客户端与双方言共存证据归 8。扩展、自定义类型及目录专用类型向通用引擎推广均不在本块范围。

[布尔类型契约](#布尔类型契约)不实现存储或类型系统，不改 Go/MySQL 真值行为；不扩展为 bool 数组、完整 cast 矩阵、运算符/聚合或协议状态机。声明解析、运算、消息恢复分别交工作流 2、4、7；本块只固定类型与 codec 的可观察交接。

[整数与无符号边界](#整数与无符号边界)不实现算术、cast 或 numeric codec，不扩写聚合、serial/identity、oid、数组及完整类型推断。工作流 2 提供声明/转换语法；4 拥有重载求值；7 拥有帧校验、错误响应和恢复；8 采集客户端与双方言证据。MySQL 原行为不因 PG 目标更改。

[精确数值契约](#精确数值契约)不实现大数表示、cast 或 codec，不改 MySQL/Go 裁剪及告警行为；不扩大到算术/聚合、索引排序、numeric 数组或协议状态机。工作流 2 接声明语法，4 接数值运算与特殊值比较，7 接消息/错误响应及恢复，8 接客户端差分。

[浮点数值契约](#浮点数值契约)不实现内部表示、cast、codec 或运算，不改 Go/MySQL 的浮点行为，不扩展浮点数组、索引、聚合或完整推导系统。类型语法归工作流 2，比较/算术归 4，输出会话配置归 5，消息错误与恢复归 7，真实客户端与双方言差分归 8。

[文本类型契约](#文本类型契约)不实现生产表示、cast、codec 或 MySQL 行为修改；比较、排序、collation 算法、LIKE/正则交工作流 4，编码会话状态交 5，目录专用类型交 6，协议分帧/错误恢复交 7，双方言客户端差分交 8。此处仅交接语义，不扩建子系统。

[字节类型契约](#字节类型契约)不实现生产表示、cast、codec，不改 MySQL BIT/HEX 或 Go 原行为。字面量解析归工作流 2，函数/比较归 4，bytea_output 状态归 5，帧长度/错误恢复归 7，客户端差分归 8；large object/lo、bytea 数组和扩展生态不在本块范围。

[日期类型契约](#日期类型契约)不实现生产表示、转换或 codec；日期字面量语法归工作流 2，日期算术/比较归 4，DateStyle、事务时间和 TimeZone 状态归 5，消息恢复归 7，客户端差分归 8。date 数组和其他时间族在各自块承接，不为扩展年份缺口扩建存储或执行子系统。

[日内时间类型契约](#日内时间类型契约)不实现生产表示或 codec，不增加 timetz、time 数组及完整时区数据库能力。字面量/类型语法归工作流 2，算术/比较归 4，TimeZone 与事务时刻归 5，目录身份归 6，消息状态恢复归 7，客户端差分归 8；本块只交接类型与拒绝契约。

[无时区时间戳契约](#无时区时间戳契约)不实现生产表示、cast、codec 或 MySQL 会话行为；timestamp 数组、完整时间运算和时区数据库不在本块。语法归工作流 2，算术/比较归 4，DateStyle/TimeZone/事务时间归 5，目录身份归 6，消息及错误恢复归 7，真实客户端差分归 8；timestamptz 独立承接，不能因 1114 映射扩大承诺。

[带时区时间戳契约](#带时区时间戳契约)不实现生产表示、cast、codec、时区数据库或完整时间运算；数组另块承接。会话状态与配置生命周期归工作流 5，语法归 2、算术/比较归 4、目录归 6、消息/恢复归 7、真实差分归 8；不扩大为完整时区子系统。

[时间间隔契约](#时间间隔契约)范围外：日期/时间戳加减 interval、比较、justify、age、extract 等函数/运算由工作流 4 承接；IntervalStyle 状态生命周期由工作流 5、SQL 声明解析由工作流 2、消息恢复由工作流 7 承接。本块只定义表示、转换及 codec 契约，不新增函数子系统，不改变 MySQL TIME 既有行为。

[通用唯一标识契约](#通用唯一标识契约)范围外：UUID生成、版本提取、排序/索引及函数运算归工作流4，目录曝光归工作流6，解析归工作流2，消息恢复归工作流7；UUID数组交基础数组块。本块不新增生成器或更改Go/MySQL UUID函数语义。

[JSON文本契约](#JSON文本契约)范围外：JSON操作、路径、构造/聚合、取值时重复键处理及Unicode解码归工作流4；jsonb规范化交下一块，数组交基础数组块。语法/定型接口归2，编码会话状态归5，目录身份归6，协议错误恢复归7，真实客户端差分归8。本块不实现生产代码、不改Go/MySQL函数、不新建可执行实现任务。

[JSONB规范值契约](#JSONB规范值契约)范围外：运算符、比较/包含、路径、索引、聚合与扩展生态归工作流4或来源范围决策；本块不实现这些能力。JSON原文由前块负责，SQL数组由基础数组块负责；语法定型归2，编码会话配置归5，目录身份归6，协议恢复归7，客户端差分归8。不扩建生产子系统，不生成执行任务。

[基础数组契约](#基础数组契约)不实现生产表示、cast或codec，不扩展自定义/复合/domain数组、下标/切片、比较/运算、索引、聚合或完整存储子系统；目录vector和ACL编码保留独立责任，不推广为普通数组。构造语法归工作流2，运算归4，array_nulls/编码状态归5，目录身份归6，消息及恢复归7，真实客户端差分归8。只定义本章类型与codec交接，不修改Go/MySQL行为。

[显式隐式转换契约](#显式隐式转换契约)只交付上述基础族转换资格与责任；完整重载、domain/用户定义cast、扩展类型、函数/算术、存储、优化器及协议状态机均不实现。bytea与整数等PG18新增局部转换不因本块列出而开放；不补建全量cast目录，不修改其他章节或Go行为。

[准备描述执行元数据契约](#准备描述执行元数据契约)不实现准备器、参数推断、缓存失效、codec或协议状态机，不扩建planner/执行器；SQL PREPARE/EXECUTE语法、事务及目录变更实现分别交工作流2、5、6，消息/恢复交工作流7。不将元数据验证推广为无副作用执行或完整缓存兼容。

### 验收

章节验收与后续交接要求逐项核对来源四步、15族四方向及VALUE/NULL/EDGE/INVALID格，沿各族验收表填精确目标、正反例、SQLSTATE、元数据、静态入口与缺口；所有格当前运行状态为未验证。TYPE-ACCEPT登记后按第1章记录提交、精确版本、环境、完整命令/退出码、有效用例数、原始产物、预期与实际；无运行用例不得填通过。META补零行、全NULL、重绑定、混合格式、typmod、缺元数据与不执行探测。文档验收另核对五公共小节、本块唯一标题、引用存在、无占位和仅第3章差异；这些检查只证明文档完成。

[规范类型与协议映射](#规范类型与协议映射)的以下能力 ID 为**待新增**验收入口，非既有测试或功能通过记录。每项按第 1 章记录方言、定型前提、输入、类型、NULL、wire 元数据、错误与当前证据；本轮全部运行行为为未验证。成功项 SQLSTATE 不适用；错误目标参照 PostgreSQL 18 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)，不以现有 io::Error 自动推导其正确传输。

| 入口与场景 | 可观察目标与反例 | 当前证据状态/责任 |
| --- | --- | --- |
| TYPE-MAP-IDENTITY：同值不同类型 | PG 定型 `1::int2`、`1::int4`、`1::int8`、`1::numeric` 及 TRUE 保持各自 OID/typlen/typmod；text '1' 不变整数。两种格式的身份一致。 | 既有 boolean bridge 测试只断言 MySQL 同列同值及 B 差异；完整场景待新增、未验证。本章定型，工作流 7 描述。 |
| TYPE-MAP-EMPTY：空结果 | `SELECT 1::int4 AS v WHERE false` 返回零行但列为 23/4/-1；Prepare/statement Describe 与 portal Describe/Execute 的类型相同，格式字段按实际模式分别验收，不能要求 text/binary 格式码相等。 | 待新增、未验证；缺元数据即使零行也拒绝，不依赖首行。 |
| TYPE-MAP-NULL：全 NULL | `SELECT NULL::int4` 返回 23/4/-1、DataRow 长度 -1；typed NULL 参数在 Bind 前后仍保留 int4 身份；空 text/bytea 与 SQL NULL 区分；JSON null 独立。 | 既有 NULL JSON 拒绝断言不能证明此场景；待新增、未验证。不确定裸 NULL 类型先按 PG 上下文取证，不能沿当前 code 6 回退视为通过。 |
| TYPE-MAP-UNSIGNED：提升 | 共享引擎/MySQL U16/U32/U64 边界，分别得到 int4/int8/numeric；u64 最大值不丢位，PG numeric 反向窄化须检查目标 unsigned 域。MySQL 描述仍保留 U。 | unsigned_binary_and_unsupported_types 有 u64 文本断言；Prepare、一致性、binary numeric 与反向边界待新增、未验证。PG 窄化溢出目标 22003，禁止截断或浮点提升。 |
| TYPE-MAP-UNKNOWN：未知或缺类型 | 直接构造未知 code、native 数量不匹配及缺必要身份；分别覆盖有行、零行、全 NULL，均不得发成功描述或猜 text；未知 PG 类型入口拒绝，已知类型但未实现 codec 明确不支持。 | 既有 missing_original_metadata 测试覆盖缺类型的有行边界，JSON 非支持含 NULL 有断言；其余待新增、未验证。项目不支持路径目标 0A000，非宣称参考 PG 也拒绝该类型。 |
| TYPE-MAP-DIRECTIONS：参数/推导/结果 | 每个已开放族跑 text/binary 参数→规范值→结果；OID 声明与上下文共同决定身份，OID 0 无法推断则拒绝，目标 42P18；反例为同载荷不同 OID、非法 text、错误 binary 长度及范围越界。 | 参数身份及 typmod 往返待新增、未验证；错误目标依原因分别 22P02、22P03、22003，字符编码 22021；具体日期等族细化错误另取证，不一律套 22P02。 |
| TYPE-MAP-NONISOMORPHIC：不可逆域 | 零日期、负/超过一天的 duration、缺时区瞬间、JSON 身份折叠、空数组无元素身份、未知数组下界不得静默包装；有明确转换才能按转换目标返回。 | 既有测试有零日期和 25:00:00 拒绝意图；其余待新增、未验证；不支持范围目标 0A000，非法已支持域的输入按各族数据错误，MySQL 原行为单独对照。 |

文档完成条件是五个公共小节均能关联本块，上述方向/类型族及五项必需边界（同值异类型、空结果、全 NULL、unsigned、未知类型）无缺项，当前事实、目标和缺证分开。文档结构通过不等于 Describe/Execute、codec、cast、SQLSTATE、双方言隔离或性能通过；本轮没有当前客户端、差分记录或有效运行用例数。

[布尔类型契约](#布尔类型契约)验收入口以下均为**待新增**，不是已执行用例；当前运行证据全部为“未验证”，成功项 SQLSTATE 不适用。每项同时采集 AsterSQL 与精确 PG 参考版本、值/NULL、类型、格式和错误；错误恢复交工作流 7，不能以 io::Error 或测试名称代替 wire 错误断言。

| 能力 ID / 场景 | 可观察目标与反例 | 已读证据与缺口 |
| --- | --- | --- |
| BOOL-IDENTITY | SELECT TRUE,FALSE,NULL::boolean；声明 bool 列含 true/false/NULL，并并列 int4 值 1/0/NULL；bool 16/1/-1，整数 23/4/-1，不能同值改型。 | common_type_oids_and_values 与 bridge 测试只覆盖部分字面量；声明列/typed NULL 待新增，未验证。 |
| BOOL-TEXT | OID 16 参数逐测完整合法词、前缀、大小写和空白；空串、o、2、truth、unknown 拒绝 22P02，非法编码 22021；输出固定 t/f。 | text_parameters_are_typed_and_reject_loss 仅 true/unknown；其余待新增，未验证。 |
| BOOL-BINARY | OID 16 载荷 00/01/02/ff 依次 false/true/true/true，结果规范化 00/01；长度 0 为 08P01，长度 2 为 22P03。非法二进制“值”反例是拒绝 02/ff 的错误实现；带 bool 身份的内部值 2 则应拒绝。 | pg_binary_native_results 只含 false 输出；输入边界及内部非法值拒绝待新增，未验证。 |
| BOOL-NULL-DESCRIBE | OID 16 的 text/binary NULL 参数、首行/全 NULL 及零行 bool 结果；ParameterDescription 为 16，statement/portal Describe 与 Execute 列身份始终 16/1/-1；NULL 长度 -1，空载荷独立拒绝。 | 参数 tp=6 与准备/执行桥接尚缺身份连续性证据；待新增，未验证。 |
| BOOL-CAST | 0::int4::boolean=false，2/-1::int4 显式转为 true；TRUE::int4=1，FALSE::int4=0；TRUE::text='true'；NULL 转换保留目标身份；WHERE 1 与整数赋 bool 列拒绝 42804。 | Datum::ToBool/test_to_bool 是 MySQL 意图，不能证明 PG；完整显式/隐式及双方言隔离待新增，未验证。 |

[整数与无符号边界](#整数与无符号边界)以下入口均为**待新增**；所有运行行为未验证。成功项 SQLSTATE 不适用，失败须断言具体 SQLSTATE，不以 is_err 代替。

| 能力 ID / 场景 | 可观察目标与反例 | 现有断言意图/缺口 |
| --- | --- | --- |
| INT-RANGE | PG int2/int4/int8 各 min/max 成功，min-1/max+1 的 text 参数及显式窄化拒绝 22003；空串/1x/1.5 拒绝 22P02；按官方输入语法补空白、符号、进制和下划线正反例。 | text_parameters_are_typed_and_reject_loss 仅 int4 最小值和上越界 is_err；其余及错误码未验证。 |
| INT-UNSIGNED | 对表内每个 signed/unsigned 域测 min/max 与外侧一值；合法 U8/U16/U24/U32/U64 依次映射 int2/int4/int4/int8/numeric。U64 的 9223372036854775808 和 18446744073709551615 精确保留，18446744073709551616→U64 及 -1→各 unsigned 拒绝。 | test_convert 有各宽度上下界及部分裁剪错误值断言；unsigned_binary_and_unsupported_types 有最大 U64 text/OID 断言。桥接拒绝、全宽 wire 和双方言隔离未验证。 |
| INT-CODEC | 每个 PG signed 宽度对 min/-1/0/1/max 做 text/binary 参数→结果，binary 长度/网络序准确；少/多一字节分别 08P01/22P03。U16/U32 扩宽两格式精确，U64 numeric 两格式精确或未开放 binary 时显式拒绝。 | pg_binary_native_results 有 i64::MIN、-2 int2、i32::MAX 输出断言；完整往返、长度错误及 numeric binary 未验证。 |
| INT-NULL-METADATA | 每个 signed 类型、各 unsigned 提升后类型含 typed NULL 参数、首行 NULL、全 NULL、空结果、混合 min/max 行；ParameterDescription 输入身份及 statement/portal Describe/Execute 结果 OID/typlen/typmod 稳定，格式选择不改变类型。 | NULL 参数现仅 is_null 断言，不证明类型连续性；U64 的零值与最大值均须保持 1700/-1/-1，全部待新增、未验证。 |
| INT-CAST-ARITH | 所有 signed 宽度间域内 cast 保值，窄化越界拒绝；负值转 unsigned 不 wrap。int4 最大值加一/各 signed 最小值取负拒绝 22003，先显式转足够宽类型后的可表示结果成功；NULL 不改结果类型，溢出不动态改 OID。 | 共享转换测试只证明 MySQL 意图；PG 重载、显隐式资格、错误码与元数据全链未验证，算术执行交工作流 4。 |

[精确数值契约](#精确数值契约)以下入口均为**待新增**，所有运行行为未验证；每项分别采集 text/binary、值与 scale、OID/typlen/typmod、NULL 和 SQLSTATE，成功项 SQLSTATE 不适用。

| 能力 ID / 场景 | 可观察目标和反例 | 已读证据与验证缺口 |
| --- | --- | --- |
| NUM-PRECISION | numeric(3,1)：±99.9 成功；±99.94 舍入为 ±99.9；±99.95 拒绝 22003。p=1/1000、s=-1000/1000 声明边界成功，p=0/1001、s=±1001 拒绝 22023；numeric(3,5) 的 .00999 成功，.009995 进位超域拒绝。 | MyDecimal 测试有 word 边界，不证明 PG 声明域；全部 PG 断言未验证。 |
| NUM-ROUND | ±1.25→numeric(3,1) 为 ±1.3；±1.249 为 ±1.2；numeric(2,-3) 的 99499/-99499→99000/-99000，99500/-99500 拒绝 22003。±1.5→int4 为 ±2；整数上下界附近舍入后超域拒绝。 | TestRoundWithHalfEven 有正负半值/负 scale，TestRoundWithTruncate 有另一模式；均未运行，不证明 PG cast。 |
| NUM-LARGE | 无约束 numeric 接收 18446744073709551615、82 位全 9 整数及 1000 位有限整数，保持精确；按 PG 实现极限验证小数点前 131072/131073 位和后 16383/16384 位的成功/22003。共享表示不足须明确 0A000 并记为兼容缺口。 | U64 仅文本断言；大精度表示、两格式往返、极限全部未验证，禁止 float8 回退。 |
| NUM-TEXT-SPECIAL | 12.340、1.25e2 精确；空串、12bad、NaNx 拒绝 22P02；非法编码 22021。NaN 保持 numeric，±Infinity 在无约束成功、constrained 拒绝 22003；当前缺表示时明确 0A000。 | 参数已有 12.340/12bad 意图；其余未验证，字符串透传不能验收特殊值。 |
| NUM-BINARY | 12.340 的字段为 (2,0,0000,3;12,3400)，结果 scale=3；U64 最大值两格式无损。构造 (2,0,0000,2;12,3456) 接收为 12.34，不能误按普通 typmod 舍入为 12.35。负值、零、特殊 sign 分别往返；非法 digit=10000/sign/scale、短载荷/额外字节分别按方向表拒绝。 | 当前 numeric binary 无实现；MyDecimal TestToBinFromBin 是 MySQL 格式，不能替代此项，全部未验证。 |
| NUM-NULL-METADATA | NULL::numeric(8,2)、typed NULL 参数、首行 NULL/全 NULL、空结果及多行不同值，均保持 1700/-1 与预定 typmod；numeric(8,2) typmod=524294，无约束/U64 为 -1。statement/portal Describe 与 Execute 身份一致，参数 OID=1700；负 scale typmod 往返不误读成正 scale。 | 既有 DECIMAL(8,2) 仅 text/OID 意图；typmod、NULL、Prepare 全链及双方言隔离未验证。 |

[浮点数值契约](#浮点数值契约)以下能力入口均为**待新增**，全部运行行为为未验证；每项分别采集两宽度、两格式、类型身份、NULL 与 SQLSTATE，成功项 SQLSTATE 不适用。codec 有限值按目标宽度位型比较，零另比符号，NaN 比类别；文本同时核对 PG 默认拼写和回读，不用统一宽容误差替代。运算另定逐操作误差模型，不沿用 codec 放宽阈值。

| 能力 ID / 场景 | 可观察目标与反例 | 已读证据与缺口 |
| --- | --- | --- |
| FLOAT-IDENTITY | 1.5::real 与 1.5::double precision 同值异型，分别 700/4/-1、701/8/-1；裸 1.5/1e0 为 numeric；float(1)/(24) 为 float4、float(25)/(53) 为 float8，float(0)/(54) 拒绝 22023。 | pg_type 有映射但无这些推导/Describe 全链断言；未验证。 |
| FLOAT-ROUNDTRIP | ±1.5、0.1、16777217 text→float4 为 16777216，float8 保留 16777217；直接 float4 输入 7.038531e-26 对照官方结果，拒绝二次舍入。默认可逆文本→同宽解码位型一致，binary 长度/字节准确。 | pg_binary_native_results 仅 1.5_f32；完整 text/binary 往返、默认拼写未验证。 |
| FLOAT-RANGE | 各宽度 ±最大有限、±最小正规及其相邻位型，±最小次正规（位型 00000001 / 0000000000000001 加符号）均成功；text 1e39→float4、1e309→float8 拒绝 22003；1e-100→float4、1e-400→float8 下溢为零拒绝。binary 对应无穷位型成功，不能按 text 溢出拒绝。 | 有网络序转换代码，无完整边界/错误断言，未验证。 |
| FLOAT-SPECIAL-ZERO | 两宽度 text/binary 的 +0/-0 符号保留，float4 binary 00000000/80000000、float8 0000000000000000/8000000000000000；NaN/±Infinity/±Inf 文本输入，输出规范词，非 NULL。NaN 与自身相等、±0 相等的 SQL 比较由工作流 4 验收。 | 仅 float8 +∞ binary 输出意图；负无穷、NaN、带符号零和比较未验证；payload 保存无承诺。 |
| FLOAT-INVALID | 空串、1x、NaNx 拒绝 22P02，非法编码 22021；float4 的 0/3/5 字节、float8 的 0/7/9 字节分别覆盖不足/额外错误；NULL 不走长度错误路径。 | 现有 binary 长度一律 22P03，与目标短载荷分类有差距；精确 wire 错误未验证。 |
| FLOAT-CAST | float4→float8→float4 保留有限位型；float8 1e39 窄化溢出、1e-100 窄化变零均 22003；±2.5→int4 在最近偶数平台为 ±2，±3.5 为 ±4；NaN/∞转整数拒绝。 | 共享转换/格式化测试不能证明 PG cast 或双方言隔离，全部未验证；类型资格交转换块。 |
| FLOAT-NULL-METADATA | 两宽度 typed NULL 参数、首行/全 NULL、空结果及跨边界多行；ParameterDescription 700/701、statement/portal Describe 与 Execute 的 OID/typlen/typmod 连续，NULL 长度 -1，格式变化不改类型。 | pg_binary_native_results 有两宽度 NULL 长度 -1 的局部输出意图；参数 NULL tp=6 与共用 f64 值盒仍不证明身份连续；完整客户端证据未验证。 |

[文本类型契约](#文本类型契约)以下 TEXT 用例族为待新增，按第 1 章登记两方言独立 case；所有运行行为**未验证**。PG 基线为 UTF-8，text/binary 参数和结果均检查，成功 SQLSTATE 不适用。MySQL 基线固定 utf8mb4、具体 collation、sql_mode 和 CHAR 取回模式，记录其独立错误码/SQLSTATE/告警，不从 PG 推导。现有 hello、NULL 和目录 char 断言仅为静态入口。

| 场景 | PG 可观察目标 | MySQL 对照与证据状态 |
| --- | --- | --- |
| TEXT-EMPTY-NULL | text/varchar(2) 空串结果长度 0；char(2) 空串成为两个空格；三类 typed NULL 长度 -1，无载荷，元数据仍保留。 | 自身空串/NULL 分开，CHAR 取回的尾空格按已记录模式验收；完整三类型两格式缺证。 |
| TEXT-MULTIBYTE | 中🙂 在 varchar(2) 成功，UTF-8 字符数 2、字节数 7；e 加 U+0301 是 2 字符，不能归一化当 1；char(3) 的中补两个空格，载荷 5 字节。 | utf8mb4 下字符限制与协议字节宽度分别断言，不复用 PG typmod；未验证。 |
| TEXT-LENGTH-CAST | 对 varchar(2)/char(2) 赋值 ab 成功，abc 拒绝 22001，ab 加一个 ASCII 空格允许裁掉；ab 后加制表符拒绝。显式 cast abc 到两者得到 ab；char(3) 输入 a 输出 a 后跟两空格。n=0、10485761 声明拒绝 22023；n=1、10485760 声明接受，不代表极大载荷已测。 | 自身 VARCHAR/CHAR 长度边界分别在严格及非严格模式采集拒绝或截断/告警，禁止用 PG cast 规则替代；全部未验证。 |
| TEXT-SPACES | text/varchar 的 a 后跟空格在两格式输出保留；char(3) 转 text/varchar 后为 a；char 比较忽略尾空格的责任交工作流 4，输出字节不先 trim。 | 分开验收 CHAR 取回和 VARCHAR 保留，以及 collation 的 PAD 属性；不能仅凭结果相同推断比较等价，未验证。 |
| TEXT-INVALID | Bind 两格式的 61 00 62、FF、C3 28 拒绝 22021；不得输出替换字符。合法多字节不能被字节截断。结果方向用构造非法内部值的候选测试检查提前拒绝与错误传输。 | NUL 的合法性、非法 utf8mb4 的错误/告警独立采集，不套 PG 拒绝；当前 PG 输入校验和结果 NUL 校验有源码证据，运行未验证。 |
| TEXT-METADATA | 声明 text/varchar(2)/char(2) 依次 25/1043/1042，typlen=-1，typmod=-1/6/6；无长度 varchar 为 1043/-1/-1，SQL char 为 1042/-1/5。检查无约束 bpchar、typed NULL、零行、重复 Execute 和两种结果格式，OID 不随值改变。 | 检查自身 type code、charset、flags、长度，不含 PG typmod。普通结果目前折 text，目录 1042 测试不能代替；全链路未验证。 |

[字节类型契约](#字节类型契约)下列 BYTEA 入口均为**待新增**，运行行为全部**未验证**；成功 SQLSTATE 不适用。固定 PG 版本、UTF-8、bytea_output，采集两格式参数→规范值→两格式结果的原字节、NULL、类型与 SQLSTATE；MySQL 单独采集 binary 类型，不套 PG 语法。

| 场景 | 可观察目标与反例 | 已读证据与缺口 |
| --- | --- | --- |
| BYTEA-EMPTY-NULL | text 空串/`\x` 与 binary 零长度均为零字节；默认 text 输出 `\x`，binary 长度 0。两格式 OID 17 NULL 参数/结果只写 -1，不变空串。 | 现有测试无完整 bytea 空/NULL 断言，未验证。 |
| BYTEA-RAW | 00、ff、c3 28、00 00 01 00、00 ff 5c 逐个往返；完整 00–ff 的 256 字节序列两格式往返逐字节相等，长度 256，不出现 ef bf bd 替换或丢前导零。binary 输入 ASCII `\x00` 保留四字节 5c 78 30 30，不能解成 00。 | 现有结果仅 00 ff[/5c]，参数仅 hex 00 ff；全字节、非 UTF-8 与完整往返缺证。 |
| BYTEA-HEX | `\xDEad`、`\xde ad`→de ad，输出 `\xdead`；`\x0`、`\xgg`、`\xa b` 拒绝 22P02。hex text 与对应 raw binary 参数结果相同。 | 当前合法空白不支持；奇数仅 is_err，无错误码，未验证。 |
| BYTEA-ESCAPE | Bind 字符 abc→61 62 63、`\000\377\\`→00 ff 5c；`\400`、`\08`、`\q`、尾部单反斜线拒绝 22P02。hex/escape 输出均回读为原字节。text 原始 NUL/非法 UTF-8 拒绝 22021，binary 对应原字节接受。 | 当前非 hex 返回 0A000 为兼容缺口；escape 配置与输出、精确错误未验证。 |
| BYTEA-METADATA | 相同 abc 载荷的 text/bytea 分别 25/17；参数描述 17，statement/portal Describe 与 Execute 均 17/-1/-1；零行、首行/全 NULL、重复 Execute、格式切换不改型。构造 metadata 与 Value::Bytes 冲突时提前拒绝，不按首行猜类型。 | 局部 OID/输出断言不能证明 charset 桥接、typed NULL 或准备全链，未验证。 |
| BYTEA-LENGTH | binary 任何合法帧长度按实际字节解释；帧声明与实际长度不符拒绝 08P01。资源上限/hex 膨胀处须检查失败，不能溢出或截断；记录实际限制后做大载荷边界差分。 | length 的 i32 转换错误入口不能证明存储/输出极限或 wire 错误码，未验证。 |

[日期类型契约](#日期类型契约)以下 DATE 入口均为**待新增**，全部运行行为**未验证**。成功项 SQLSTATE 不适用；按第 1 章记录具体参考版本、DateStyle、TimeZone、事务时间、两格式原字节、类型、NULL 与错误。每个合法值覆盖 text/binary 参数→规范值→text/binary 结果四种组合；特殊词先变规范日期再比较，不要求保留输入拼写。

| 能力 ID / 场景 | 可观察目标与反例 | 已读证据与缺口 |
| --- | --- | --- |
| DATE-CALENDAR | 2024-02-29、2000-02-29 成功；2023-02-29、1900-02-29、2024-04-31、月 00/13、日 00/32 拒绝 22008。构造内部 2023-02-29，text/binary 均拒绝，不能格式选择改变合法性。 | 当前只有一对闰日参数 is_ok/is_err，结果粗校验有差距，精确错误未验证。 |
| DATE-EPOCH | 1999-12-31、2000-01-01、2000-01-02 对应 ff ff ff ff、00 00 00 00、00 00 00 01；1970-01-01 为 -10957 日。四组合值和字节一致，改变 TimeZone 不移动已存日期。 | 仅 -1 日结果局部断言，参数 binary 未开放，完整往返未验证。 |
| DATE-RANGE | 日序 -2451545/2145031948 及相应文本成功，外侧各一天拒绝 22008；1 BC 与 1 AD 跨界无外部零年，10000-01-01 合法并保留。表示不足明确 0A000 且记缺口，不能把合法 PG 值裁到 9999。 | 共享 u64 字段和 chrono 路径不证明全域；边界和 BC 未验证。 |
| DATE-ZERO | 输入 0000-00-00、0000-01-01、2024-00-01、2024-01-00 拒绝 22008；桥接 MySQL 零/非法日期两种结果格式均拒绝，不映射 epoch、infinity 或 NULL。MySQL 单独在固定 sql_mode 下保持原错误/告警行为。 | 当前结果仅零/粗范围检查，双方言隔离和 SQLSTATE 未验证。 |
| DATE-SPECIAL-STYLE | -infinity/infinity 两格式分别往返 80 00 00 00/7f ff ff ff，非 NULL；epoch→1970-01-01。固定事务与时区核对 now/today/昨日/明日及 Prepare 后读取时机；allballs、空串、garbage 拒绝 22007。ISO 基线及 MDY/DMY 歧义输入按官方值，输出回读同日。 | 现有日期入口未覆盖非有限/相对词/DateStyle，均未验证。 |
| DATE-NULL-METADATA | typed NULL 参数、NULL::date、首行/全 NULL、零行及重复 Execute；参数描述 1082，statement/portal Describe 与 Execute 1082/4/-1，结果格式切换不改身份；NULL 长度 -1，不能混同 binary 0 日或空串。 | 有局部 NULL 输出意图，参数 tp=6 连续性及准备全链未验证。 |
| DATE-INVALID-BINARY | 非 NULL 0/3 字节不足拒绝 08P01，5 字节额外拒绝 22P03；四字节 -2451546/2145031949 拒绝 22008，两个哨兵接受；帧声明与实长不符拒绝 08P01。text 非法 UTF-8/NUL 拒绝 22021。 | date binary 参数当前 0A000，长度/域/恢复及精确 SQLSTATE 均未验证。 |

[日内时间类型契约](#日内时间类型契约)以下 TIME 入口均为**待新增**，运行行为全部**未验证**；成功项 SQLSTATE 不适用，精度收敛告警单独记录。固定具体 PG 构建、编码、TimeZone 与 typmod，合法值覆盖两格式参数→两格式结果四种组合，采集原字节、值、NULL、元数据和错误；MySQL 独立记录 sql_mode、Fsp、类型及自身错误/告警。

| 能力 ID / 场景 | 可观察目标与反例 | 静态证据和未验证项 |
| --- | --- | --- |
| TIME-DAY | 00:00:00、23:59:59.999999、24:00:00 分别为 0、86399999999、86400000000 微秒；24:00 不等于零点，binary 为各整数八字节网络序。24:00:00.000001、25:00:00、12:60:00、12:00:61 拒绝 22008。 | 已有 1 微秒结果局部断言；24:00 参数既有拒绝与目标相反，text 完整校验及四组合未验证。 |
| TIME-PRECISION | time(0) 的 12:34:56.5→12:34:57；time(3) 的 12:34:56.1234→.123、.1235→.124；23:59:59.999999 到 time(0)/(3) 均进位 24:00:00。p=0/6 保留声明，负 p 拒绝 22023，p=7 告警 22023 并归 6。超过六位输入如 12:34:56.1234567 按参考微秒舍入为 .123457，不能直接拒绝。 | 共享 MySQL 舍入断言不证明 PG 输入/typmod；当前非整微秒拒绝，全部 PG 精度边界未验证。 |
| TIME-TEXT-ZONE | 04:05→04:05:00、040506→04:05:06、04:05 PM→16:05:00、allballs→00:00:00；04:05:06+08:00 输入普通 time 后仍 04:05:06，切换 TimeZone 不移动它且 OID 始终 1083。 | 当前固定格式不证明官方输入集/时区处理，均未验证；timetz 请求明确拒绝 0A000。 |
| TIME-DURATION | MySQL -00:00:01、25:00:00、838:59:59 在自身规则下单独验收，桥接 PG time 两格式均 0A000；PG 24:00:00 不因旧 hour<24 规则被误拒绝。反向显式桥接保留 24 小时与微秒，禁止暗变 interval。 | 仅已有 25 小时结果 is_err 和共享负 duration 数值断言；双方言全链与精确错误未验证。 |
| TIME-NULL-METADATA | NULL::time(3)、typed NULL 参数、首行/全 NULL、零行、多行和重复 Execute 均保持 1083/8/3；无声明 typmod=-1，time(6) 为 6；参数描述 1083。两种结果格式与 statement/portal Describe 一致，NULL 长度 -1，零点长度 8 的 binary 全零。 | 有局部 binary NULL 输出；参数 tp=6 与固定 typmod=-1 不证明身份/修饰符连续，完整流程未验证。 |
| TIME-INVALID | 空串/garbage→22007，非法 UTF-8/NUL→22021；binary 0/7 字节→08P01、9 字节→22P03，八字节 -1/86400000001/i64 极值→22008；帧实长不符→08P01。构造内部 12:60:00，text/binary 均拒绝，格式不改变合法性。 | 当前 binary 参数未开放、text 只查小时；所有精确错误和协议恢复未验证。 |

[无时区时间戳契约](#无时区时间戳契约)以下 TS 用例均为**待新增**，运行行为全部**未验证**；成功 SQLSTATE 不适用，精度声明告警另记。按第 1 章固定参考构建、UTF-8、DateStyle、TimeZone、事务时间、p；合法值覆盖 text/binary 参数→text/binary 结果四组合，记录原字节、规范值、NULL、元数据及错误。MySQL 独立记录 code=7/12、Fsp、sql_mode、写入/读取时区和自身错误/告警。

| 能力 ID / 场景 | 可观察目标及反例 | 静态证据及缺口 |
| --- | --- | --- |
| TS-EPOCH-FRACTION | 1999-12-31 23:59:59.999999、2000-01-01 00:00:00、同日 .000001 分别为 i64 -1/0/1 网络序；六位微秒逐位保留。输入 .1234567 归 .123457；timestamp(3) 的 .1234→.123、.1235→.124。epoch 两侧 ±500000 微秒收敛 p=0 为 ±1000000，不能按日历秒对负值舍错方向。 | 只有 -1 微秒结果及六位参数局部断言；现有七位拒绝与目标有差距，全部往返未验证。 |
| TS-CALENDAR-PRECISION | 2024-02-29 合法；2023-02-29、1900-02-29、2024-04-31、月13、分钟60 拒绝 22008；2024-12-31 23:59:59.999999 到 p=0/3 进位 2025-01-01。p=0/6 接受，负 p 拒绝22023，p=7 告警22023归6。构造非法内部日期/时分秒，两格式同拒绝。 | text 粗查与 binary chrono 校验不等域；日期边界、精度及精确错误未验证。 |
| TS-RANGE-ZERO | 精确有限上下端微秒成功，-211813488000000001、9223371331200000000 拒绝22008；BC/AD无年份0，10000年不裁剪。0000-00-00、0000-01-01、2024-00-01、2024-01-00 均22008；MySQL零/非法日期桥接两格式同拒绝。合法PG值承载不足须0A000并记缺口。 | 年1–9999限制与目标有差距；原date零值测试不证明timestamp零值，全域和桥接未验证。 |
| TS-ZONE-IDENTITY | TIMESTAMP '2004-10-19 10:23:54+02' 仍为10:23:54/OID1114；已存2024-03-10 02:30:00在UTC、Asia/Shanghai、America/New_York切换后日历字段与binary不变，不能因DST缺口当瞬间纠正。MySQL分别用DATETIME/TIMESTAMP写后切换时区，记录自身结果与桥接前提，缺前提拒绝0A000。 | code7/12映射不足以证明会话隔离；列名含timezone或相同文本不构成1184证据，未验证。 |
| TS-SPECIAL | -infinity/infinity 两格式分别往返i64最小/最大且非NULL；epoch→1970-01-01 00:00:00。固定事务核对now/today/昨日/明日，在Prepare常量与Bind参数各自读入时转换，不随后续Execute重新解释已存值。 | 当前无特殊值/相对词断言，未验证。 |
| TS-NULL-METADATA | NULL::timestamp(3)、typed NULL参数、首行/全NULL、零行、多行及重复Execute，ParameterDescription=1114；statement/portal Describe与Execute均1114/8/3，无声明typmod=-1。两格式NULL长度-1，binary epoch长度8全零，空串拒绝；不由首行或格式改型。 | 有局部NULL结果意图；tp6与固定typmod=-1不足以证明身份连续，完整客户端流程未验证。 |
| TS-INVALID-LENGTH | 空串/garbage→22007，非法UTF-8/NUL→22021；binary非NULL 0/7字节→08P01、9字节→22P03，八字节域外非哨兵→22008，帧实长不符→08P01；错误后恢复由工作流7验收。 | binary参数当前0A000，所有精确错误及恢复未验证。 |

[带时区时间戳契约](#带时区时间戳契约)下列 TZ 用例全部**待新增、未验证**。固定参考构建、DateStyle、TimeZone、tzdata、p、编码与读入阶段；每个合法值覆盖 text/binary 参数到 text/binary 结果四组合，记录规范瞬时、原字节、元数据、NULL 和错误。成功 SQLSTATE 不适用，告警单列；已有 1114 测试只作静态对照。

| 用例 | 可观察验收目标 | 证据状态 |
| --- | --- | --- |
| TZ-SAME-INSTANT | 上述 +00/+08 两输入相等，binary 都是八字节零；同墙钟 +00/+08 则相差八小时，不比较原输入拼写。 | 无 1184 独立断言，未验证。 |
| TZ-SESSION | 同一已存/已绑定瞬时 UTC 显示午夜+00，Shanghai 显示08:00+08；二进制不变。无偏移文本绑定时记录当时区域，改区后不重新解释存量值；解析时特殊词另记录求值时机。 | PgSession 无 TimeZone 字段；状态交接及真实客户端未验证。 |
| TZ-DST | New_York 春季02:30→07:30Z、秋季01:30→06:30Z；显式-04→05:30Z，显式-05→06:30Z；text 重显示与 binary 瞬时一致。 | 以官方 B.2 为预期，未运行参考端或本地。 |
| TZ-PRECISION-RANGE | epoch±1微秒保真，p=0 的 ±500000→±1000000；七位输入舍入及跨日进位。有限两端成功，端外一微秒22008，偏移归一跨边界亦拒绝；BC无年份0、扩展年份、infinity/-infinity 两格式一致，零日期22008。 | 当前 CoreTime/chrono 与全域有差距；全部未验证。 |
| TZ-CAST-BRIDGE | timestamp 午夜在UTC/Shanghai解释得到不同瞬时；重复小时两个瞬时转无时区可同值，不能声称互逆。NULL/特殊值按目标契约；缺MySQL来源时区0A000，不改Go原有行为。 | 只有Go显式字段转换意图，无PG转换/桥接实测。 |
| TZ-NULL-METADATA | NULL::timestamptz(3)、typed NULL参数、首行/全NULL/零行、多行及重复Execute，ParameterDescription=1184；statement/portal Describe与Execute保持1184/8/3，未声明-1；切换格式或时区不改身份。NULL长度-1区别于epoch八字节零。 | 当前tp6及1114映射不能证明，未验证。 |
| TZ-INVALID | garbage/空串22007，2月30日及归一溢出22008，非法编码/NUL22021；binary 7/9字节分别08P01/22P03，帧错误08P01；未知区名按PG输入错误分类对照，不能回退本地默认时区。合法能力缺口0A000单独登记，非成功替代。 | 当前统一不支持不是完整错误分类；恢复由工作流7验收，未验证。 |

[时间间隔契约](#时间间隔契约)以下 IV 场景均为**待新增、未验证**。固定 PG 构建、UTF-8、IntervalStyle、fields/p；合法输入覆盖 text/binary 参数到 text/binary 结果四组合，采集原字节、三分量、NULL、元数据和 SQLSTATE。成功 SQLSTATE 不适用；告警单记。text 回读按分量验收，不只按显示或比较结果判等。

| 场景 | 可观察目标及反例 | 当前证据/责任 |
| --- | --- | --- |
| IV-COMPONENTS | `1 month`→(1,0,0)，`30 days`→(0,30,0)，`1 day`→(0,1,0)，`24 hours`→(0,0,86400000000)；四组合保持原维度。`1.5 months`→(1,15,0)，明确区别输入小数单位归整与存量整数月折算。 | 月→720小时的既有断言证明不能直接复用；PG 往返未验证。日期算术另交工作流4。 |
| IV-SIGNS-STYLE | postgres 下 `-1 month +2 days -03:04:05.123456`→(-1,2,-11045123456)；纯负、零与四种输出样式回读保真。`-1 2:03:04` 在 postgres/sql_standard 分别按独立符号/统一负号核对。 | 仅 MySQL 单一总量负号测试，PG 混合符号与会话分流未验证。 |
| IV-PRECISION-FIELDS | interval(3) 的 ±1.2345秒→±1.235秒；interval(0) 的 ±0.5秒→±1秒；p=0/6、七位小数微秒收敛与舍入进位按参考端。`1 day 2:03:04` HOUR TO MINUTE→(0,1,7380000000)，YEAR 对 `1 year 2 months`→12个月。负p拒绝22023、p=7告警后归6。 | 不以 MySQL Fsp 断言替代；PG 修饰符与精确元数据未验证。 |
| IV-RANGE-SPECIAL | binary 独立月/日 i32 极值、时间 i64 极值与相邻值保真；三分量全最小/全最大专门往返 -infinity/infinity，非 NULL；其他混合极值保持有限。文本 `2147483648 months` 拒绝22015；单位累加/收敛溢出按阶段核对22015/22008，禁止绕回或饱和。 | 现有 MySQL TIME 小域不能证明 PG 全域；边界与错误阶段未验证。 |
| IV-NULL-METADATA | NULL::interval(3)、typed NULL参数、首行/全NULL/零行、多行和重复Execute保持 ParameterDescription=1186；statement/portal Describe与Execute均1186/16/编码后的typmod，未声明-1。两格式NULL=-1；binary零间隔长度16全零，不能猜为text。 | 当前tp6提前返回不证明身份连续；准备全链未验证。 |
| IV-INVALID | 空串/garbage→22007、非法UTF-8/NUL→22021；binary非NULL 0/15字节→08P01、17字节→22P03，帧声明/实长不符→08P01。错误后恢复由工作流7核验；不能用当前0A000冒充目标输入校验。 | 所有精确错误及真实客户端未验证。 |
| IV-BRIDGE | MySQL -51小时仅在显式约定下桥接(0,0,-183600000000)；含月/日的反向桥接或超MySQL域均0A000，不能折日或截断；typed NULL保留目标身份。MySQL自身值/错误按原sql_mode另验收。 | 当前通用映射仍1083；桥接资格及双方言隔离未验证。 |

[通用唯一标识契约](#通用唯一标识契约)以下 UUID 场景均为**待新增、未验证**；固定PG精确构建及UTF-8，合法值覆盖text/binary参数到text/binary结果四组合，比较规范16字节、输出、NULL、元数据和错误。成功SQLSTATE不适用；不以字符串相等替代类型断言。

| 场景 | 可观察验收目标 | 当前证据状态 |
| --- | --- | --- |
| UUID-TEXT | 标准、大写、无连字符、花括号及每四位连字符变体均得到同一16字节；输出统一小写36字节格式；显式text↔uuid往返保持值及目标类型。 | 官方uuid.c静态依据；独立测试未见上述断言，未运行。 |
| UUID-BITS | 全零、全ff、首/尾字节01/80及00112233样本两格式往返；任意version/variant位均接受；无字节交换。 | 固定字节布局有源码依据，四组合全链未验证。 |
| UUID-INVALID | 空串、31/33位、g字符、奇数位、错位/尾连字符、单侧花括号、URN、前后空白拒绝22P02；非法UTF-8走22021；binary长度0/15走08P01、17走22P03；不截断。 | 现有2950缺分支，目标诊断及协议恢复均未验证。 |
| UUID-NULL-METADATA | typed NULL参数、NULL::uuid、首行/全NULL、零行、多行、重复Execute仍保持参数2950及结果2950/16/-1；两格式NULL长度-1，全零binary长度16。 | 现有tp6提前返回不能证明身份保真；Describe/Execute全链未验证。 |
| UUID-INFERENCE-BRIDGE | 声明列/显式cast/2950定型结果一致；OID0从明确上下文定型，歧义拒绝沿推导契约；普通UUID外观text及16字节bytea保持自身身份，无依据桥接0A000。 | 现有MySQL推导与PG出口分离证据；目标推导、桥接及双方言回归未验证。 |

[JSON文本契约](#JSON文本契约)下列 JSON-TEXT 场景全部**待新增、未验证**；固定PG精确构建及UTF-8配置，每个合法值覆盖text/binary参数到text/binary结果四组合，分别记录输入原字节、保存文本及结果载荷，不能只比较解析后值树。成功SQLSTATE不适用，错误同时记录阶段与SQLSTATE。

| 场景 | 可观察目标及反例 | 当前证据状态 |
| --- | --- | --- |
| JSON-TEXT-PRESERVE | 输入 ` { "b":1, "a":2,"b":3 } `及制表/换行变体，输出原样，两个b成员、顺序和外侧空白都保留；`1.230e+02`不改写123。 | BTreeMap与重序列化暴露差距；既有规范文本往返不能证明保留，未验证。 |
| JSON-TEXT-NUMBER | 9007199254740993、18446744073709551616、1e1000000以及长小数两格式逐字节保留，不能舍入或以numeric域拒绝；01/+1/NaN/Infinity拒绝22P02。 | appendBinaryNumber的f64回退不能证明；大数PG往返未验证。 |
| JSON-TEXT-SCALAR | `42`、`true`、`false`、`"x"`、`[]`、`{}`均为非NULL json；根字符串结果保留引号，不能调用json_unquote。 | 现有MySQL容器/标量断言不证明OID114，未验证。 |
| JSON-TEXT-UNICODE | `"中"`、`"\u4e2d"`分别原样保存，`"\u0000"`及`"\uD800"`不在纯存取时被解码/拒绝；`"\uZZZZ"`及原始未转义控制字符22P02，非法UTF-8/原始NUL22021。 | serde值树不是PG词法保留证据；处理函数或jsonb阶段另验收，未验证。 |
| JSON-TEXT-INVALID | 空串、纯空白、多根值`{} []`、`[1,]`、未闭合字符串、注释均22P02；binary前缀字节01加合法文档不能当jsonb接受；错误后恢复交7。 | TestParseBinaryFromString仅有MySQL错误意图，PG两格式诊断未验证。 |
| JSON-TEXT-NULL-METADATA | typed NULL参数、NULL::json、首行/全NULL/零行/多行/重复Execute保持ParameterDescription114及结果114/-1/-1。SQL NULL两格式长度-1，JSON null长度4且字节6e756c6c，JSON字符串`"null"`带引号长度6；格式码切换不改身份。 | 已有NULL JSON拒绝断言不是支持证据，准备全链未验证。 |
| JSON-TEXT-CAST-BRIDGE | text显式转json校验并保留；反向转text得到原文；不自动把普通text、MySQL245或jsonb冒充原文json。缺有证据桥接时0A000；MySQL独立按原Go契约验证。 | 转换资格及双方言回归未验证，不用输出字符串相同证明类型相同。 |

[JSONB规范值契约](#JSONB规范值契约)以下JSONB场景全部**待新增、未验证**。合法值覆盖text/binary参数×text/binary结果四组合，记录精确PG构建、编码、参数OID/格式、结果OID/长度/typmod/格式、语义值及原始载荷；去掉binary的01前缀后应与同一值的text输出一致。错误记录阶段和SQLSTATE，成功SQLSTATE不适用。

| 场景 | 可观察目标 | 证据状态 |
| --- | --- | --- |
| JSONB-DUP-ORDER | `{"aa":1,"b":2,"aa":3}`保留aa=3，固定REL_18_0输出`{"b": 2, "aa": 3}`；嵌套重复键、`"a"`与`"\u0061"`同键最后值；输入换序不改变规范对象。 | 已读MySQL往返测试未断言此PG键序/解码重复键；未验证。 |
| JSONB-NUMBER | `18446744073709551617`及高精度小数无舍入；`1.230e-5`输出`0.00001230`，`1.00`保留小数尾零；超PG numeric域如`1e200000`拒绝22003，NaN/Infinity及非法数字22P02。PG域内不能承载时明确0A000。 | appendBinaryNumber走f64不足；未验证精度、scale及诊断。 |
| JSONB-SCALAR-NEST | `{}`、`[]`、嵌套`[null,{"x":[true,"中",1]}]`及独立`1`、`"x"`、false、null均保持种类；`[2,1,2]`保持顺序和重复元素。 | TestBinaryJSONMarshalUnmarshal仅MySQL意图；PG往返未验证，深度资源边界需按参考端测，不能照搬100。 |
| JSONB-UNICODE | `"中"`与`"\u4e2d"`输出同值，`"\uD83D\uDE00"`正确合并；`"\u0000"`及孤立代理对22P05；`"\uZZZZ"`22P02；非法UTF-8/原始NUL在编码阶段22021。 | PG类型输入比json文本严格；阶段/诊断均未实测。 |
| JSONB-NULL-METADATA | SQL NULL长度-1，JSON null的text为`null`、binary为`01 6e 75 6c 6c`；空集/首行NULL/全NULL、重绑定均保持参数3802与结果3802/-1/-1，Describe/Execute一致，不变text。 | unsigned_binary_and_unsupported_types仅证明当前JSON结果拒绝；typed NULL未验证。 |
| JSONB-WIRE-INVALID | binary长度0缺版本拒绝08P01；仅01无文档22P02；00/02/ff版本拒绝并采集参考SQLSTATE；01后非法/截断JSON、多根值、尾随垃圾22P02；声明长度超可用帧08P01。 | 当前无3802 codec；不能用0A000替代最终参考诊断，所有长度/版本场景未验证。 |
| JSONB-CAST-BRIDGE | text/json显式转jsonb规范化；含json可保存但jsonb不能接受的转义/超域数字在转换时拒绝；反向json/text只得规范文本；MySQL opaque/time扩展不自动映射。 | 方言cast资格、MySQL不变回归和PG错误阶段未验证。 |

[基础数组契约](#基础数组契约)以下ARRAY场景全部**待新增、未验证**；SQL构造须先满足第2章交接，codec可直接使用带数组OID的`SELECT $1 AS v`参数场景。每个合法值覆盖text/binary参数×text/binary结果四组合，记录精确参考构建、编码、array_nulls、原始载荷、参数及结果OID/typmod/格式、形状/下界/元素NULL。成功SQLSTATE为“不适用（成功）”；错误记录阶段、实际SQLSTATE和协议恢复，不能以目录测试替代。

| 场景ID | 可观察目标与边界 | 当前证据状态 |
| --- | --- | --- |
| ARRAY-SHAPE | int4[] `{1,2}`保存一维[2]/下界[1]；`{{1,2},{3,4}}`保存二维[2,2]/[1,1]；顺序1,2,3,4，二者结果OID1007，维数不改变类型。 | 目录编码仅一维；普通SQL/参数四组合未验证。 |
| ARRAY-LOWER | int4[] `[0:1][-2:-1]={{1,2},{3,4}}`四组合往返保留shape/lower；text仍带维度前缀，binary每维下界不改1。 | 现有binary_array固定下界不能作保真证据；未验证。 |
| ARRAY-EMPTY-NULL | `{}`为非NULL零维空int4数组、OID1007、binary为12字节头0/0/23；NULL::integer[]外层-1；`{NULL}`为一维[1]、一个元素-1；三者不同。空集/首行NULL/全NULL及重绑定Describe/Execute均保持1007/-1/-1。 | pg_binary_native_results只断言目录空int2数组及NULL外层；通用typed NULL和稳定元数据未验证。 |
| ARRAY-TEXT-ESCAPE | text[] `{"NULL",NULL,"","a,b","a\\b","a\"b","中"}`区分NULL字样、NULL元素、空串及转义；小写未引用null为NULL；引用/反斜线不丢失，不按逗号简单切割。 | 通用输入、输出引用及四组合未验证；固定array_nulls=on。 |
| ARRAY-TEXT-INVALID | int4[] `{{1,2},{3}}`、`[0:2]={1,2}`、`{1,}`、未闭合引号/转义均22P02；`{x}`22P02、`{2147483648}`22003；不补NULL、不截断。 | 输入codec缺失；文本路径诊断未验证，ARRAY构造路径另采阶段。 |
| ARRAY-WIRE-TYPE | 外层1007配头元素23成功；改头为内建25拒绝42804，即使空数组也校验元素身份；外层误报1009不得依值猜为int4[]。 | pg_oid_test只目录对象OID唯一性；数组OID配对及拒绝未验证。 |
| ARRAY-WIRE-LENGTH | ndim=-1、flags=2、元素length=-2/超剩余/未消费完整拒绝22P03；缺头/长度字段按08P01边界；维长负值、乘积/下界溢出、7维54000、元素数不符和尾随垃圾分别记录参考诊断。flags=0且NULL元素单列参考接受性，输出应规范反映NULL。 | 未验证；不得让长度错误越过载荷边界，未采集码的分支不冒充通过。 |
| ARRAY-ELEMENT-CAST | int4[]→int8[]在允许显式转换下保留shape/lower/NULL、结果1016且头20；超域/非法元素按元素契约失败，无部分结果；同值text[]不能保留1007冒充原身份。 | cast资格及元素精度/时区继承待转换块；PG往返与MySQL隔离回归未验证。 |

[显式隐式转换契约](#显式隐式转换契约)待新增CAST用例族如下，全部PG/双方言运行均未验证。每条成功项须比对值、目标OID/typmod及NULL标志，拒绝项须比对SQLSTATE、阶段、是否有P和原位置；simple Q与扩展路径分别采集，不用测试名称或编译替代行为。

| 用例 | 可观察目标与反例 | 证据状态 |
| --- | --- | --- |
| CAST-LEGAL-INVALID | `'12'::int4`→12/OID23；`'12x'::int4`拒绝22P02；`'2147483648'::int4`拒绝22003，无裁剪行。 | 现有test_str_to_num的前缀接受恰为非等价证据；PG未验证。 |
| CAST-CONTEXT | `1::boolean`→true/16；`SELECT 1 WHERE 1`拒绝42804；int4列赋值boolean拒绝，显式true::int4→1。另int4→int2赋值合法但超域拒绝22003。 | E/A/I分离目标；MySQL独立验证原真值/告警，均未运行。 |
| CAST-UNKNOWN-NULL | `SELECT NULL::int4`返回typed NULL/23；`SELECT '1' UNION SELECT 2`定为int4，而`SELECT '1'::text UNION SELECT 2`拒绝42804；单列全unknown NULL集合定为text/25。 | 源码NULL短路不能证明定型；未验证。 |
| CAST-COMMON | `SELECT 1.2 UNION SELECT 1`为numeric/1700；`CASE WHEN true THEN 1 ELSE 2.2 END`为numeric；`VALUES (1),(2.2)`同列numeric；`SELECT NULL UNION SELECT NULL UNION SELECT 1`因左侧先定text而拒绝42804。 | 共同算法、逐对顺序、CASE候选及空结果描述均未验证。 |
| CAST-PARAMETER | Parse省略OID的`SELECT $1::int4`推定23，Bind文本12/binary整数12/NULL均保持结果23；`SELECT $1 IS NULL`无上下文目标42P18。用INSERT到int4、text两列的`VALUES ($1,$1)`采集不一致推断42P08；显式OID25的`SELECT $1 + 1`拒绝重载42883，不能因绑定“1”改变资格。 | 当前普通Parse OID0为0A000；目标参数冲突优先级、位置及两格式均未验证。 |
| CAST-ROUND-OVERFLOW | numeric `1.235::numeric(4,2)`→1.24，`99.995::numeric(4,2)`舍入后越界22003；`2.5::numeric::int4`→3、`(-2.5)::numeric::int4`→-3；float半值按浮点族单独采集，不能混用numeric规则。 | 现有decimal钳制为MySQL证据；PG舍入与溢出未验证。 |
| CAST-FAMILY-METADATA | 固定UTC/Shanghai比较timestamp↔timestamptz；json→jsonb规范化再返json不得恢复原文；int4[]→int8[]保留非1下界、shape及元素NULL，结果1016/元素20；text→UUID合法与非法按UUID块；bytea→text按bytea输出。 | 前序族验收关联；元数据在空行/全NULL/重绑定及text/binary均一致，全部未验证。 |
| CAST-LENGTH-BRIDGE | `'abcd'::varchar(2)`→ab；向varchar(2)列直接赋值'abcd'拒绝22001。unsigned最大值提升numeric保真；MySQL零日期/负或超24小时duration不得冒充PG日期/time，无依据桥接0A000。 | 沿文本、整数与时间块；双方言隔离及无部分写入均未验证。 |

[准备描述执行元数据契约](#准备描述执行元数据契约)以下META入口全部为**待新增、未验证**。按第1章锁定PG精确构建、客户端、编码、会话/目录前提，采集Parse OID、两种Describe原包、Bind格式/值和Execute载荷及SQLSTATE；成功SQLSTATE不适用。MySQL按自身Prepare协议独立回归，不比较OID相等。

| 场景 | 可观察目标与负例 | 当前证据状态 |
| --- | --- | --- |
| META-EMPTY-NULL | `SELECT NULL::int4 AS v WHERE false`零行、`SELECT NULL::int4 AS v`全NULL，以及预建int4列首行NULL后接1；两Describe均23/4/-1/一列，Execute零行或NULL=-1，首行不定型。 | 目录零行有静态断言；通用三场景全链未验证。 |
| META-PARAM | `SELECT $1::int4 AS v`分别用无OID、[0]、[23] Parse，目标参数23、结果23/4/-1；`SELECT $1 IS NULL`无OID目标42P18。额外显式text参数经合法cast时参数25与结果23须分别记录，不能强制相等。 | 普通未指定OID当前0A000；显式23表谓词有静态意图，参数化投影未验证。 |
| META-REBIND | 同一int4语句依次新Bind 1、2、NULL及两种参数格式；描述不变、值分别对应；重复/乱序$n按编号绑定。域外int4拒绝22003，不能升级结果OID。 | 重绑定/乱序有局部静态断言，表达式/范围/typed NULL连续性未验证。 |
| META-FORMAT | 同一两列int4/text语句创建text、binary及混合portal；statement格式全0，各portal格式与Bind匹配，OID/typlen/typmod/列数相同。切换portal及重复Execute不串格式；错误格式数量/码拒绝08P01。 | binary局部载荷断言不覆盖完整portal隔离，未验证。 |
| META-TYPMOD | `SELECT NULL::numeric(8,2), NULL::varchar(3), NULL::timestamp(3)`及非NULL/零行变体；OID1700/1043/1114，typlen -1/-1/8，typmod分别为前序族规定的编码，statement/portal一致。 | encode_formats固定-1为已知差距，未验证。 |
| META-MISSING-CHANGE | 注入缺native_types、数量不符、执行行宽不符及准备/执行OID或typmod冲突，均应在DataRow前明确拒绝；预建单列int4表后Parse `SELECT *`，ALTER增加列再新Bind，固定结果变化目标0A000；另独立夹具改int4为int8同样拒绝。重新Parse可建立新描述。 | 缺元数据/数量静态断言及描述比对代码已读；变化与DDL全链未验证，不保证DDL自身已支持。 |
| META-NO-PROBE | 用执行调用计数或受控有副作用函数夹具观测Parse及两种Describe：执行原查询次数0，无数据副作用；只有Execute触发一次，分批重复Execute不重新求值。分析/规划合法的常量折叠不等于执行查询探测。 | Describe不调用执行闭包为静态证据，计数/副作用及准备内部链未验证；夹具为待新增，不引用现有函数名。 |

### 后续细分

章节验收与后续交接仅保留候选边界：内部表示先确认各族身份、值域、typmod及typed NULL；cast依赖表示与E/A/I上下文；参数codec依赖稳定参数OID、元素codec和会话前提；结果codec依赖相同规范值、固定结果描述及portal格式。按族重新核实依赖与占用后由用户决定实施划分，本轮不生成执行任务。后续推荐工作流4承接类型/转换契约，工作流8补真实运行证据；15族、转换和元数据的未验证状态不得随文档完成升级。未发现需修改其他章节的契约冲突。

[规范类型与协议映射](#规范类型与协议映射)仅给候选边界：先取证 FieldType/NativeType 能保留的身份、修饰符与参数生命周期，再决定最小内部表示补充；cast 候选依赖该身份并先分方言确定转换规则；参数 codec 候选依赖确定的参数类型及值域；结果 codec 候选依赖相同的准备/执行列类型契约。各族顺序服从本章后续文档，不生成可执行代码任务、不递归扩建共享内核。协议消息/恢复的实现由工作流 7 独立承接。

来源步骤覆盖：来源任务步骤 1 的“规范类型↔MySQL/PG 映射、推导、参数/结果、NULL”由两个方向表与共同约束覆盖；本编号任务要求的方案比较由保真缺口决策覆盖；验收五边界由 TYPE-MAP-IDENTITY/EMPTY/NULL/UNSIGNED/UNKNOWN 覆盖。来源步骤 2–4 的逐族完整语义、cast 与精确 codec 留在本章其余编号文档块，本块映射索引不替代那些交付。第 1 章证据规则与第 2 章类型/数组语法交接保持只读，未发现需要改写前序契约的冲突。

[布尔类型契约](#布尔类型契约)候选边界为：内部表示先验证 bool 身份能随字面量、声明列、有类型 NULL 和参数跨 Prepare/Execute 保留；cast 依赖该身份并区分显式、赋值与隐式上下文；参数 codec 依赖已定型 OID，补齐文本词集与二进制接收域；结果 codec 依赖同一列身份并保持 Describe/Execute 一致。这里只描述候选边界与依赖，不生成可执行实现任务。

布尔块来源覆盖：来源步骤 2 的 bool 族由方向表覆盖；步骤 3 的 bool 显式/隐式边界由转换段及 BOOL-CAST 覆盖；步骤 4 的 text/binary、NULL、非法输入、元数据一致性由 BOOL-TEXT/BINARY/NULL-DESCRIBE 覆盖。与前块的“不能自动将非零整数视为 bool”一致，显式 int4 cast 和 OID 16 单字节接收是有来源的限定场景；未发现需改动第 1/2 章或来源任务的冲突。

[整数与无符号边界](#整数与无符号边界)候选边界：内部表示先验证声明宽度、signed/unsigned 与 typed NULL 跨 Prepare/Execute 保留；cast 依赖身份及方言上下文，独立核对窄化与负值；参数 codec 依赖已定型 OID，补输入语法及错误分类；结果 codec 依赖稳定结果元数据，U64 binary 另依赖 numeric 契约。本块不生成可执行实现任务。来源步骤 2 的整数族由值域表及方向表覆盖，步骤 3 的 unsigned/溢出由桥接及算术交接覆盖，步骤 4 的格式/NULL/非法输入/元数据由 INT-RANGE/UNSIGNED/CODEC/NULL-METADATA/CAST-ARITH 覆盖。未发现与前序布尔限定场景或第 1/2 章冲突；功能、参考端与性能均未实测。

[精确数值契约](#精确数值契约)候选边界：先核定 MyDecimal 与 cast Decimal 可保真域及规范身份的 precision/scale/特殊值缺口，再决定内部表示；cast 依赖该表示和方言资格，保留 PG 舍入/错误及 MySQL 原行为分流；参数 codec 依赖已定型 OID 和上下文 typmod，结果 codec 依赖同一身份及稳定 Describe 元数据。这里只给边界与依赖，不生成可执行任务。来源步骤 2 的 numeric 族由身份与方向表覆盖，步骤 3 的精度溢出/cast 由 NUM-PRECISION/ROUND 覆盖，步骤 4 的格式、非法输入、NULL 与元数据由 NUM-TEXT-SPECIAL/BINARY/NULL-METADATA 覆盖；NUM-LARGE 承接前块 U64 与更大精度差距。未发现前块或第 1/2 章冲突；本轮无功能、性能及参考端实测。

[浮点数值契约](#浮点数值契约)候选边界：内部表示先核对 KindFloat32、共享 f64 值盒与 NativeType 的定宽不变量及 typed NULL 保真；cast 依赖该不变量和方言上下文，分别核对舍入/范围与特殊值；参数 codec 依赖确定 OID，直接按目标宽度解析并分类错误；结果 codec 依赖同一列身份和输出配置，分别输出目标宽度文本及网络位型。这里只给边界和依赖，不生成实现任务。来源步骤 2 的浮点族由身份/方向表覆盖，步骤 3 的舍入和溢出由 FLOAT-RANGE/CAST 覆盖，步骤 4 的格式、NULL、非法输入和元数据由其余验收入口覆盖。与 numeric 的精确性和半值规则分开，与第 1/2 章术语及前序块无冲突；没有功能或性能实测。

[文本类型契约](#文本类型契约)候选边界与依赖为：内部表示先核对身份、声明字符长度、charset/collation 与 typed NULL 的连续性；cast 在该身份基础上区分显式截断、赋值拒绝和 char 去尾空格；参数 codec 依赖 OID 定型与 SQL 目标约束，分别校验编码/NULL；结果 codec 依赖同一列身份和编码状态，保持 OID/typmod 与补空格字节一致。这里只描述边界，不生成实现任务。来源步骤 2 由类型/方向表覆盖，步骤 3 由长度 coercion/cast 规则覆盖，步骤 4 由 TEXT 用例族覆盖；与第 1/2 章及前五块无契约冲突。无功能、兼容性或性能实测。

[字节类型契约](#字节类型契约)候选边界：内部表示先核对 bytes、binary charset、OID 17 及 typed NULL 跨桥接保真；cast 依赖该身份与方言资格；参数 codec 依赖已定型 OID，补 hex 空白/escape 并隔离 raw binary；结果 codec 依赖同一列身份和 bytea_output 配置，保持可逆及 Describe/Execute 一致。这里只给边界与依赖，不生成实现任务。

字节块来源覆盖：来源步骤 2 由值域/映射及四方向表覆盖；步骤 3 由转换边界段覆盖，完整资格交转换块；步骤 4 由 BYTEA 六项验收覆盖。与前块 text 禁止 NUL、bytea 允许 NUL 及第 1/2 章语法/证据分层一致，无需改其他章节；无功能、兼容性或性能实测。

[日期类型契约](#日期类型契约)候选边界：内部表示先核对 date 身份、有限日序、BC/扩展年份/非有限值及 typed NULL 的保真缺口；cast 依赖该身份和方言上下文，不复用 MySQL 宽松解析作为 PG 规则；参数 codec 依赖确定 OID、DateStyle 与特殊词解析时机；结果 codec 依赖相同规范值与稳定列元数据，两格式共享完整日历校验。这里只描述边界和依赖，不生成可执行实现任务。

日期块来源覆盖：来源步骤 2 的 date 族由日历/值域及四方向表覆盖；步骤 3 的零日期、不等价边界和转换交接由拒绝路径与 DATE-ZERO/RANGE 覆盖；步骤 4 的格式、NULL、非法输入及元数据由 DATE 七项验收覆盖。与第 1/2 章及前七块无须改写的冲突；精确下端采用官方源码日序而非概略年份表，后续实施须锁定参考版本复核。无功能、性能或参考端实测。

[日内时间类型契约](#日内时间类型契约)候选边界：内部表示先核对独立 time 身份、含 24:00 的微秒日内域、p 与 typed NULL，评估复用 Duration 是否能保真；cast 依赖方言身份并先拒绝负/长 duration，不自动提升；参数 codec 依赖定型 OID/SQL 约束，补 PG 文本与 binary 校验；结果 codec 依赖同一规范值和稳定元数据，共用完整值域检查。这里只描述候选边界与依赖，不生成可执行任务。

日内时间块来源覆盖：步骤 2 的 time 族由四方向表覆盖；步骤 3 的 duration 不等价、精度及转换由 TIME-DURATION/PRECISION 覆盖；步骤 4 的格式、NULL、非法输入和 Describe/Execute 一致性由 TIME-DAY/TEXT-ZONE/NULL-METADATA/INVALID 覆盖。与第 1/2 章及日期块无须改写的冲突；timetz 保持来源范围外。无功能、性能或真实参考端实测。

[无时区时间戳契约](#无时区时间戳契约)候选边界：内部表示先核对无时区身份、微秒有限域/特殊值、p 与 typed NULL 的保真，避免借 MySQL TIMESTAMP 标签发明瞬间；cast 依赖该身份并明确 MySQL 会话前提及 PG 转换资格；参数 codec 依赖已定型 OID、SQL p、DateStyle 和读入时机；结果 codec 依赖同一规范值及稳定 Describe 元数据，两格式共享完整值域校验。这里只给候选边界与依赖，不生成可执行实现任务。

无时区时间戳块来源覆盖：步骤 2 的 timestamp 族由四方向表与有限域覆盖；步骤 3 的精度、零日期及不引入时区由 TS-EPOCH-FRACTION/CALENDAR-PRECISION/RANGE-ZERO/ZONE-IDENTITY 覆盖；步骤 4 的格式、NULL、非法输入、长度与 Describe/Execute 由 TS-SPECIAL/NULL-METADATA/INVALID-LENGTH 覆盖。与第 1/2 章及前序块无须改写的冲突，timestamptz 留下一块承接；无功能、性能或参考端实测。

[带时区时间戳契约](#带时区时间戳契约)候选边界：内部表示先核定独立瞬时身份、域/特殊值、p 与 typed NULL；cast 依赖规范身份和工作流 5 提供的区域上下文，区分墙钟解释与瞬时显示；参数 codec 依赖 OID 定型、SQL精度及解析时会话配置；结果 codec 依赖同一规范值、稳定 Describe 和显示区域，binary 不能经有损墙钟中转。这里只描述候选边界和依赖，不生成可执行实现任务。

带时区时间戳块来源覆盖：步骤 2 由瞬时定义、元数据及四方向表覆盖；步骤 3 的精度/时区和MySQL不等价由 TZ-SESSION/DST/PRECISION-RANGE/CAST-BRIDGE 覆盖；步骤 4 由四格式组合、TZ-SAME-INSTANT/NULL-METADATA/INVALID 覆盖。与第1/2章及无时区块的身份、时区前提一致，无需改写来源或其他章节；没有功能、性能或18.6参考端实测。

[时间间隔契约](#时间间隔契约)候选边界：内部表示先核定独立 interval 身份、三分量、非有限值、fields/p 与 typed NULL 保真；cast 依赖身份和方言资格，禁止用总秒数替代日历分量；参数 codec 依赖已定型 OID、SQL 修饰符和 IntervalStyle；结果 codec 依赖相同规范值与稳定 Describe 元数据，直接输出三分量。这里只描述边界和依赖，不生成可执行任务。

时间间隔块来源覆盖：步骤2的interval族由身份、范围与四方向表覆盖；步骤3的精度/不等价转换由IV-PRECISION-FIELDS/RANGE-SPECIAL/BRIDGE覆盖；步骤4的格式、NULL、非法输入及元数据由IV-COMPONENTS/SIGNS-STYLE/NULL-METADATA/INVALID覆盖。与第1/2章及前序时间块无须改写的冲突；没有功能、性能或参考端实测。

[通用唯一标识契约](#通用唯一标识契约)候选边界：内部表示先确定独立UUID身份、固定16字节和typed NULL；cast依赖此身份及统一方言资格；参数codec依赖已定型2950，分别实施词法解析与固定字节解码；结果codec依赖规范值和稳定Describe元数据，输出规范文本或原字节。这里只描述边界与依赖，不生成可执行任务。

UUID块来源覆盖：步骤2由规范身份及四方向表覆盖；步骤3的显式转换与不等价桥接由UUID-TEXT/INFERENCE-BRIDGE覆盖（精度、溢出、时区不适用于固定128位类型）；步骤4由UUID-BITS/INVALID/NULL-METADATA和四组合往返覆盖。第1章证据等级、第2章语法交接及前序text/bytea身份规则保持一致，无需改写其他块。文档完成不代表UUID功能、性能或18.6差分通过。

[JSON文本契约](#JSON文本契约)候选边界：内部表示先验证独立json身份、原文载体与typed NULL连续性，保留现有MySQL BinaryJSON；cast依赖该身份及方言资格；参数codec依赖已定型114与编码上下文，校验但不重建；结果codec依赖同一原文和稳定Describe元数据，两格式直接输出文本。这里只描述边界和依赖，不生成实现任务。

JSON文本块来源覆盖：步骤2的JSON族由原文身份与五方向表覆盖；步骤3的精度/不等价转换由JSON-TEXT-NUMBER/CAST-BRIDGE覆盖，时区与数组维度不适用于本标量族；步骤4由四格式组合及PRESERVE/SCALAR/UNICODE/INVALID/NULL-METADATA覆盖。与第1章证据规则、第2章语法交接和前序类型身份契约一致，无需改写其他章节；jsonb独立承接，不宣称功能、性能或18.6差分通过。

[JSONB规范值契约](#JSONB规范值契约)候选边界：内部表示先确立独立jsonb身份、精确numeric及scale、解码字符串、对象去重和typed NULL；cast依赖该表示和方言转换资格，复用json原文校验入口时仍执行jsonb严格校验。参数codec依赖定型3802与编码上下文，binary先验版本再解析；结果codec依赖同一规范值、参考输出与稳定元数据，两格式共享输出语义。这里只描述边界及依赖，不创建代码任务。

JSONB块来源覆盖：步骤2由独立身份、值域及五方向表覆盖；步骤3由NUMBER/UNICODE/CAST-BRIDGE覆盖精度与非等价转换，时区及SQL数组维度不适用；步骤4由四格式组合及DUP-ORDER/SCALAR-NEST/NULL-METADATA/WIRE-INVALID覆盖。沿第1章证据规则、第2章语法交接及批次14原文责任，无跨块冲突；全部PG能力仍待实现取证，不宣称功能或性能通过。

[基础数组契约](#基础数组契约)候选边界：内部表示先保留元素身份/typmod、矩形shape、下界及两层NULL；cast依赖元素族与方言资格，逐元素转换不能丢形状。参数codec依赖定型数组OID、元素codec和会话编码，先校验头/边界再解码；结果codec依赖同一规范值、稳定元数据与元素输出，两格式共享shape/lower/NULL语义。目录binary_array仅作局部取证入口，不预设直接复用。这里只给候选边界与依赖，不生成可执行任务。

基础数组块来源覆盖：步骤2由元素/数组身份、值域和五方向表覆盖；步骤3由ARRAY-SHAPE/LOWER/ELEMENT-CAST覆盖维度、下界及转换，精度/时区继承元素族；步骤4由四格式组合及EMPTY-NULL/TEXT-ESCAPE/TEXT-INVALID/WIRE-TYPE/WIRE-LENGTH覆盖元数据、NULL与非法输入。与第1章证据规则、第2章数组树形/定型交接和前序各标量族一致，无需改其他章节；数组开放逐族取证，不以本块文档完成声称功能可用。

[显式隐式转换契约](#显式隐式转换契约)候选边界：内部表示先承接各族身份、typmod、unknown来源及typed NULL；cast依赖该身份和连接方言，先决策E/A/I、共同类型与参数约束，再复用等价值转换；参数codec依赖分析得到的稳定OID，结果codec依赖目标表达式身份及固定描述。这里只交接候选边界与依赖，不生成实现任务。来源步骤3的显式/隐式、精度溢出、时区、数组维度/下界及MySQL不等价边界由方向矩阵与CAST八组覆盖；步骤4相关NULL、格式和准备/执行描述交批次18承接生命周期。与第1章证据规则、第2章语法及前16块一致，无需改写其他章节；功能、性能、18.6参考端与MySQL回归未实测。

[准备描述执行元数据契约](#准备描述执行元数据契约)仅给候选边界：内部表示承接前序各族身份/typmod与typed NULL，并核定准备描述的可信来源；cast依赖定型信息，负责参数约束和结果身份；参数codec依赖稳定参数OID与逐参数格式；结果codec依赖固定结果描述及portal格式。生命周期/失效协作工作流5–7，不生成可执行任务或重建共享子系统。

准备描述执行块来源覆盖：步骤1由五阶段方向表覆盖规范类型、参数/结果及NULL分离；步骤4由META七组覆盖格式、NULL、值域/非法输入与Prepare/Describe/Execute一致性。与第1章证据规则、第2章参数语法及前17块一致，未发现需修改其他章节或来源的冲突；文档完成不表示功能实现，所有运行行为、性能及18.6/MySQL参考端均未实测。

## 4. 方言函数与运算符契约

### 模块

#### 三层身份与契约模板

本块只建立本章公共记录规则及一个标量候选，承接第 1 章的能力 ID/证据分级、第 2 章的连接方言和名称环境、第 3 章的规范类型、E/A/I 转换资格与准备/执行描述。后续契约实例统一引用下方模板，再填写自身差异；不复制一份可能漂移的公共规则。本轮文档完成不表示函数已支持。

静态取证基线为 `d98e9f395b83d26fc2a5bef0aecc232d3bc376c9`（2026-10-23），以下源码和独立测试均只阅读、未执行。两个目标包均无 `doc.go`；通过仓库 RustCodeGraph 索引定位真实源码，没有以 `.rs.md` 说明代替接线证据。

| 入口与实际符号 | 已核实的责任与证据限制 |
| --- | --- |
| [builtin.rs](../pkg/expression/builtin.rs)：`formal_registry::FunctionClassRegistry`、`registeredFunctionClass::getFunction`、`invoke_factory`、`CORE_BUILTIN_FACTORIES`、`BUILTIN_SPECS` | 注册表键是名称字符串，类检查参数数量后调用工厂；有名称但无工厂时明确返回“complete implementation is not linked”。`abs` 在规格表中，已读核心工厂表无对应条目；本块未证明另有动态工厂接入，不据名称宣称可求值。该表也未证明 PG 按 namespace、参数类型及方言选重载。 |
| [lib.rs](../pkg/expression/lib.rs)；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`、`NewFunction`、`ScalarFunction::EvalInt` / `EvalReal` / `EvalDecimal` | crate 挂载正式 builtin、scalar 和独立测试；构造沿 `funcs.get → getFunction → RetType`，标准构造可常量折叠，求值委托具体 builtin。此为应优先复用的标量链；没有证明 PG 身份、宽度和 typed NULL 已贯通。 |
| [builtin_math.rs](../pkg/expression/builtin_math.rs)：`abs_signature`、`abs_int` / `abs_uint` / `abs_real` / `abs_decimal` | 按共享 EvalType/unsigned 区分局部算法，`abs_int` 检查 i64 最小值；不包含 PG int4 返回身份及 int4 最小值专属检查的完成证据，不能用 i64 容器自动实现 int4 重载。 |
| [builtin_test.rs](../pkg/expression/builtin_test.rs)：`test_is_null_func`；[builtin_registry_aster_unit_test.rs](../pkg/expression/builtin_registry_aster_unit_test.rs)：`builtin_registry_matches_complete_go_function_table`；[builtin_math_test.rs](../pkg/expression/builtin_math_test.rs)：`abs_ceil_floor_and_round_cover_normal_boundary_and_overflow`；[builtin_math_16_aster_unit_test.rs](../pkg/expression/builtin_math_16_aster_unit_test.rs)：`abs_round_and_metadata_match_go_branches` | 公共测试委托注册表套件，不能将保留的 Go 注释当已执行断言；注册表测试主要核对名称，数学测试实际断言局部值、i64 溢出及 unsigned 签名意图，均不证明 PG SQL/协议全链。 |
| [pg_result.rs](../pkg/server/pg_result.rs)：`pg_type`、`encode_formats`、`value_text`、`value_binary`；[pg_types_test.rs](../pkg/server/pg_types_test.rs)：`common_type_oids_and_values`、`native_metadata_bridge_preserves_boolean_and_mysql_columns` | wire 归 server，按 ColumnInfo/NativeType 而非单元值选 OID，typmod 当前固定 -1，NULL 编码 -1；boolean 测试意图表明相同 MySQL 列和值仍需不同 native 身份。普通字面量测试预期 1 的 OID 为 20，并不是 PG int4 函数签名证据。 |
| [builtin_math.go](../pkg/expression/builtin_math.go)：`absFunctionClass.getFunction`；[builtin_math_test.go](../pkg/expression/builtin_math_test.go)：`TestAbs` | MySQL/Go 原契约依据：Int/Decimal/Real 分派、unsigned 与返回精度信息，以及 nil、正负整数/浮点断言意图；不运行 Go 测试，不将其替换为 PG 宽度规则。 |

#### 注册工厂与方言可见性

本块静态基线为 `e4915aee9239f58320be5691a5716d085e703519`（2026-10-23），已确认前块模板和第 1–3 章交付存在；`pkg/expression/doc.go` 不存在。按 RustCodeGraph 的 `status`、`explore`、定点 `node` 读取以下真实链路及独立测试，未执行测试，也未据注册规模推断支持。

| 入口/符号 | 实际责任及证据边界 |
| --- | --- |
| [builtin.rs](../pkg/expression/builtin.rs)：`FunctionClassRegistry::from_specs` / `get`、`funcs`、`extensionFuncs` | 内建表由 `BUILTIN_SPECS` 建类，并把 `CORE_BUILTIN_FACTORIES` 注入工厂表；扩展表初始为空。键为字符串，已读结构没有方言、namespace 或参数类型组成的重载键，不能证明连接方言过滤已接入。 |
| 同文件：`IsFunctionSupported`、`VerifyArgsWrapper`、`GetBuiltinList` | 前者仅查内建表；参数 wrapper 对不存在的类返回 Ok，假设调用者已查支持；列表合并内建/扩展并过滤展示名。这三种结果均不能独立证明签名合法、方言可见或可执行，扩展存在也不能由仅查内建的接口否定。 |
| 同文件：`registeredFunctionClass::getFunction`、`invoke_factory`、`registerBuiltinFactory` | 常规注册类先验参数数目，再取工厂；`invoke_factory` 对 GROUPING/FTS 有 planner bridge 特例，其余查动态工厂表，缺失时返回 registered-but-not-linked 错误。规格中的 abs 不在核心工厂表；尚未排除其他初始化动态注册，故只能作为受控缺工厂夹具候选，不能宣称生产调用必失败。 |
| [lib.rs](../pkg/expression/lib.rs)；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`、`NewFunction`、`NewFunctionBase`、`ScalarFunction::Eval` / `EvalInt` / `EvalString` | crate 挂载并导出正式构造与求值器。常规构造先查 `funcs`，再查 `extensionFuncs`，未知名按 CurrentDB 返回 No database selected 或 FUNCTION … does not exist；之后调用类工厂、接收返回类型并包装 ScalarFunction。标准构造会折叠，Base 不折叠，求值按类型委托具体 builtin；特殊 CAST 等提前分支不能当普通函数候选解析器。 |
| [builtin_test.rs](../pkg/expression/builtin_test.rs)：`test_is_null_func`；[builtin_registry_aster_unit_test.rs](../pkg/expression/builtin_registry_aster_unit_test.rs)：`builtin_registry_matches_complete_go_function_table`、`builtin_argument_ranges_match_go_unbounded_and_optional_rules`、`migrated_string_control_and_like_factories_match_go_runtime` | 公共入口委托独立注册表套件；后者分别断言名称存在/未知名、参数边界及未知名 wrapper 的 Ok、通过正式 NewFunctionBase 构造 IFNULL(NULL,'fallback') 并 EvalString 得到非 NULL fallback。名称计数与这些局部断言均不是 PG 可见性或双方言协议通过证据，本轮只读断言意图。 |

#### 重载与隐式转换选择

本块静态基线为 `83aea884a48778cc6032d28f1897acbdc56125b9`（2026-10-23），前置批次 1/2 的提交 `e4915aee92` / `83aea884a4` 及五节正文已核对。`pkg/expression/doc.go`、`pkg/server/doc.go` 均不存在。通过 RustCodeGraph status、explore、query 和定点 node 阅读真实源码及独立测试，未执行测试。

| 入口/符号 | 已读事实与限制 |
| --- | --- |
| [builtin.rs](../pkg/expression/builtin.rs)：`registeredFunctionClass::getFunction`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl` | 参数数目检查后调用 `invoke_factory`；构造沿 `funcs.get / extensionFuncs.Load → getFunction → getRetTp → ScalarFunction`，普通函数先调用 `typeInferForNull`（IF/IFNULL/NULLIF/ROW 除外），再按 fold 策略折叠。该共享流程不能作为 PG unknown、typed NULL 或按精确类型选重载的证据。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`formal_registry::BuildCastFunction`；[core_support.rs](../pkg/expression/core_support.rs)：`BuildCastFunctionWithCheck`、`WrapWithCastAsInt` | 正式 CAST 固定目标元数据、继承可空性/字符属性并构造 DynamicKind::Cast，非 JSON 可折叠；wrapper 委托正式入口，ETInt 可直接返回原表达式。这是 MySQL EvalType 级包装，不能证明 PG int4/int8 身份或 E/A/I 资格。 |
| [builtin_cast.rs](../pkg/expression/builtin_cast.rs)：`FieldKind` / `FieldType`、`BuildCastFunctionWithCheck`；[builtin_cast_4_aster_unit_test.rs](../pkg/expression/builtin_cast_4_aster_unit_test.rs)：`null_and_identity_casts_are_preserved` | 局部模型在构建时保存目标类型、可空性和 in_union；测试断言 NULL 转换保留可空性、求值仍 NULL、同 Int 包装恒等。该 API 与正式 trait Expression 入口分别取证，不能自动等同或据此宣称 PG 隐式资格已接线。 |
| [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)：`parse_bind_execute_sync` | 断言目录 Q 与 portal Describe/Execute 一致、零行仍有描述、显式 OID 的重复参数按编号绑定；省略 OID 的普通 `WHERE id = $1` 明确断言 0A000。仅有断言意图，没有本轮运行证据，也没有本块五组 PG 重载全链证据。 |

#### 等价共享与别名门槛

本块取证基线 `e128b200ac2d251b79f73eaad8a4f7bc9ef19c69`（2026-10-23）；批次 1–3 的提交及正文已核对，`pkg/expression/doc.go` 不存在。RustCodeGraph status、explore、query、定点 node 用于定位源码和独立测试，图未提供名称字面量位置时以 rg 补充。全部为静态阅读，未运行行为验证。

| 真实入口/符号 | 事实与证据限制 |
| --- | --- |
| [builtin_compare.rs](../pkg/expression/builtin_compare.rs)：`coalesce`、`aggregate_compare_type`、`cast_datum` | 对已物化 Datum 列表推导聚合类型，再扫描首个非 NULL，遇 Error 传播、选中后转换。Datum::Error 哨兵可表达短路意图，不证明 SQL 子表达式延迟求值、PG 分析期类型选择或 typed NULL 元数据。 |
| [builtin_control.rs](../pkg/expression/builtin_control.rs)：`infer_type_for_control`、`derive_charset` | 全 Null 类型得到 Null/binary，其他参数按本地 EvalType、flags、flen/decimal 及字符属性聚合；字符串分支可继承首个非 binary 参数 collation。这些 MySQL 局部规则不能代替 PG unknown→text、共同类型或显式 collation 冲突规则。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`invoke_factory`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`；[lib.rs](../pkg/expression/lib.rs) | COALESCE 规格为 1..无限参数；已读核心工厂表无 coalesce/nullif，缺工厂会拒绝。动态注册未全面排除，不能断言所有生产调用必失败。正式构造先查类、推导 NULL（NULLIF 属例外）、调用工厂并取得 RetType，随后可折叠；crate 挂载 compare/control kernel 和独立测试。未证明 PG 特殊表达式或 NULLIF 重写已贯通。 |
| [builtin_compare_test.rs](../pkg/expression/builtin_compare_test.rs)、[builtin_compare_7_aster_unit_test.rs](../pkg/expression/builtin_compare_7_aster_unit_test.rs)：`coalesce_matches_go_first_non_null_and_error_order`、`string_comparison_uses_build_context_collation_snapshot` | 前者断言 NULL/1→1、全 NULL→NULL、首个非 NULL 后 Error 被跳过、前置 Error 传播；后者通过正式 NewFunctionBase/EvalInt 检查 EQ 的构建上下文字符比较。后者不是 NULLIF 或 PG collation 全链证据。 |
| [builtin_control_test.rs](../pkg/expression/builtin_control_test.rs)：`if_and_ifnull_cover_truth_conversion_short_circuit_and_null`、`control_type_inference_keeps_width_flags_and_string_fixups` | 验证意图为 IF/IFNULL 未达 Error、全 NULL、类型 flags 与宽度；不能由 IFNULL 局部断言批准 COALESCE 别名，更没有 NULLIF 比较/提升的独立 SQL 证据。 |

#### 同名异义分流

本块取证基线 `6e15d36d648129eaf2694da2e5d3edc9274f1ff3`（2026-10-23）；批次 1–4 的提交和正文已核对，累计依赖满足。`pkg/expression/doc.go` 不存在。使用 RustCodeGraph status/explore/定点 node 阅读源码，名称字面量和模块挂载位置以 rg 补充；只读静态证据，不运行测试。

| 入口及真实符号 | 已读事实与限制 |
| --- | --- |
| [builtin_string.rs](../pkg/expression/builtin_string.rs)：`STRING_FUNCTION_SPECS`、`verifyStringFunctionArgs`、`concat` | CONCAT 规格为 1..无限；局部拼接接收已转换的可空字符串，任一 None 返回 None。不负责 PG 任意类型输出、共同元数据或正式 SQL 调用，不能直接充作 PG concat。 |
| [builtin_compare.rs](../pkg/expression/builtin_compare.rs)：`greatest` / `least` → `extremum` → `aggregate_compare_type` / `cast_datum` / `compare_non_null` | 非空局部列表遇 NULL 即返回 NULL，NULL 前 Error 传播，后方 Error 跳过；比较消费本地 collation/时态模式。局部空列表拒绝但单参数可计算，不能以此覆盖正式规格的两参数下限，也不证明 PG 共同类型/忽略 NULL。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`invoke_factory`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`；[lib.rs](../pkg/expression/lib.rs) | 规格 CONCAT 为 1..无限、GREATEST/LEAST 为 2..无限；已读核心工厂表无三者条目，动态注册未全面排除，不能断言所有生产调用失败。正式链仍是名称查类→参数检查→工厂→RetType→ScalarFunction/可选折叠→求值，缺工厂明确拒绝；crate 挂载局部实现及独立测试。未证明 PG 分流接线。 |
| [builtin_string_test.rs](../pkg/expression/builtin_string_test.rs)：`TestConcat` → [builtin_string_25_aster_unit_test.rs](../pkg/expression/builtin_string_25_aster_unit_test.rs)：`run_string_parity_suite` / `numeric_rounding_repeat_and_concat_preserve_edge_cases` | 真断言为 a/空串/中→a中、a/None→None；保留的 Go 混型 case 注释不是 Rust 运行断言，不能证明数值转字符串或 SQL/协议全链。模块名经 lib.rs 的 path 属性映射，不能误引不存在的同名文件。 |
| [builtin_compare_test.rs](../pkg/expression/builtin_compare_test.rs)：`greatest_and_least_preserve_go_evaluation_order_before_null`；[builtin_compare_7_aster_unit_test.rs](../pkg/expression/builtin_compare_7_aster_unit_test.rs)：`greatest_and_least_preserve_null_and_temporal_string_rules` | 真断言意图覆盖整数极值、ci 字符比较、NULL、Error 顺序和日期/字符串；均直接调用局部函数，无 PG 正式入口证明。 |
| [builtin_string.go](../pkg/expression/builtin_string.go)：`concatFunctionClass.getFunction` / `builtinConcatSig.evalString`；[builtin_compare.go](../pkg/expression/builtin_compare.go)：`resolveType4Extremum` / `greatestFunctionClass.getFunction`；[builtin_compare_test.go](../pkg/expression/builtin_compare_test.go)：`TestGreatestLeastFunc` | MySQL/TiDB 原契约依据：CONCAT 转 ETString 并保留长度/packet 限制；极值按聚合类型和时态模式构造，Go 测试含字符串/整数混型预期。这里只读，不运行 Go、不修改原语义。 |

#### 时间截断与提取

本块静态基线为 `af99cc260d`（2026-10-23）；已核对批次 1–5 的提交及正文，累计依赖满足。`pkg/expression/doc.go`、`pkg/server/doc.go` 不存在。使用 RustCodeGraph status、explore、query、定点 node 阅读以下真实源码与独立测试；图未定位名称字面量时以 rg 补查，不以说明文件或注册名称证明支持，全部测试均未运行。

| 入口/符号 | 实际责任与证据限制 |
| --- | --- |
| [builtin_time.rs](../pkg/expression/builtin_time.rs)：`extract` | 局部 kernel 输入 NaiveDateTime、返回 i64；SECOND 只取整数秒，MICROSECOND 只取秒内小数，另有 MySQL YEAR_MONTH 等复合单位。不能承接 PG SECOND 的小数、MICROSECONDS 的整秒加小数、numeric/float8 身份、timestamptz 或 interval 三分量。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`extract_factory`、`CORE_BUILTIN_FACTORIES`、`CoreBuiltinKind::Extract`、`ExtractMode` | 正式 extract 已注册为两个参数，构造按单位及第二参数 EvalType 选择 Datetime/Duration/DatetimeFromString、插入 MySQL cast，返回 ETInt；evalInt 传播 NULL 并委托 `types_dependency::time::ExtractDatetimeNum/ExtractDurationNum`。不是调用上述局部 kernel 的证据；已查本文件未见 date_trunc/date_part 字面量，不能据此排除其他动态接线或宣称完整系统必失败。 |
| [scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`、`NewFunctionBase`；[lib.rs](../pkg/expression/lib.rs) | 正式构造经类工厂取得 RetType 包装 ScalarFunction，标准入口可折叠、Base 不折叠；crate 挂载时间 kernel、公共时间测试及独立注册表测试。应复用此链并在方言签名处分流，不以 MySQL ETInt 覆盖 PG 返回身份。 |
| [builtin_time_test.rs](../pkg/expression/builtin_time_test.rs)：`test_convert_tz`；[builtin_time_30_aster_unit_test.rs](../pkg/expression/builtin_time_30_aster_unit_test.rs)：`timezone_conversion_honors_named_and_fixed_offsets`、`run_time_parity_suite` | 前者调用 suite 并实际断言微秒跨日转换、Amsterdam 秋季重叠及春季缺口；后者断言固定/命名时区及非法区名。保留 Go 注释不是新增断言，这些 MySQL 转换断言也不是 PG date_trunc 或 DST 日桶证据。 |
| [builtin_registry_aster_unit_test.rs](../pkg/expression/builtin_registry_aster_unit_test.rs)：`extract_factory_matches_go_temporal_dispatch_and_pb_signatures` | 通过 NewFunctionBase/EvalInt 断言 YEAR→2020、HOUR→12、DAY_SECOND→260304，并检查三个 TiPB 分支；只有 MySQL 正式工厂局部断言意图，不能证明 PG numeric、时区、SQL 语法或协议。 |
| [pg_types_test.rs](../pkg/server/pg_types_test.rs)：`common_type_oids_and_values` | Q 路径断言 DATETIME→1114、DECIMAL→1700 等 OID/文本；没有本块 date_part→701、timestamptz→1184、interval→1186 的函数结果或 binary/typed NULL 专项证据。协议出口责任沿第 3 章，不能用已有类型字面量断言替代函数全链验收。 |

#### 格式化函数契约

本块静态基线为 `a485e8e82c0e977ae44600bee20982d421f20464`（2026-10-23），批次 1–6 提交与五个全局小节中的前序正文齐全，累计依赖满足。`pkg/expression/doc.go`、`pkg/server/doc.go` 不存在；经 RustCodeGraph status、explore、query、定点 node 阅读真实源码，图未覆盖名称字面量和模块路径映射时用 rg 补查。下列均为静态取证，测试未运行。

| 入口/符号 | 已核实事实及证据限制 |
| --- | --- |
| [builtin_time.rs](../pkg/expression/builtin_time.rs)：`date_format` | NaiveDateTime 到 String 的 MySQL `%` 掩码内核，含英文月份/星期、微秒及 ISO 周字段；不携带 PG timestamp/timestamptz/interval 身份、数值模板、locale 或 NULL 上下文。不能机械替换模板后作为 to_char。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`registeredFunctionClass::getFunction`、`invoke_factory` | date_format 规格为 2 参数；已读核心工厂表无 date_format、to_char，本文件未找到 to_char 名称。常规类验证参数数目后取动态工厂，缺失时报 registered-but-not-linked；未排除其他初始化接线，不能据此断言所有 SQL 必失败。 |
| [scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`；[lib.rs](../pkg/expression/lib.rs) | 正式构造依次查 funcs/extensionFuncs、调用 getFunction、取返回类型并包装 ScalarFunction，标准构造可折叠；应沿此链增加方言签名策略，不另建 PG 求值器。lib 将时间测试别名挂载到独立 30 号文件。 |
| [builtin_time_test.rs](../pkg/expression/builtin_time_test.rs)：`test_date_format`、`builtin_date_format`；[builtin_time_30_aster_unit_test.rs](../pkg/expression/builtin_time_30_aster_unit_test.rs)：`run_time_parity_suite` | 前者实际只调用通用套件，原 Go DATE_FORMAT 的 NULL/模板断言与 helper 主体仍在注释；后者覆盖时间运算、TIME_FORMAT、时区等，不能当作 DATE_FORMAT 专项或 PG to_char 已执行证据。 |
| [builtin_string.rs](../pkg/expression/builtin_string.rs)：`formatByLocale`；[builtin_string_25_aster_unit_test.rs](../pkg/expression/builtin_string_25_aster_unit_test.rs)：`construction_metadata_char_format_load_and_weight_match_go` | FORMAT 是数值、decimals、可选 locale 的独立 MySQL 算法，未知 locale 回退 en_US 并报告识别标志；测试断言 en_US/de_DE 输出意图。不支持据此推断 PG D/G/L、模板语法或会话 locale。 |
| [pg_types_test.rs](../pkg/server/pg_types_test.rs)：`common_type_oids_and_values` | Q 路径对 'hello'/NULL 断言 OID 25，对普通时间/数值有其他 OID 断言；没有 to_char 重载、模板、扩展协议和 binary 结果专项证据。 |

#### 数组函数契约

本块静态基线为 `2168dce847743526d87fa6f54e401c61f6b7f32c`（2026-10-23）。批次 1–7 的连续提交及五节正文已核对，前置完成任务文件已删除，无需恢复。`pkg/types`、`pkg/expression`、`pkg/server` 均无 `doc.go`。按 RustCodeGraph status/explore/定点 node 阅读以下真实源码和独立测试；图未定位名称字面量时以 rg 补充。全部是静态取证，测试未运行。

| 入口/实际符号 | 已核实责任与证据限制 |
| --- | --- |
| [datum.rs](../pkg/types/datum.rs)：`Datum`、Kind 常量、`Clone` / `Copy` / `IsNull` | 统一盒含 Kind、字节、collation 及 Any 载荷；已读 Kind 集合未见通用 PG 数组标签，结构未直接承载元素身份、shape/lower 和元素 NULL 位图。JSON/VectorFloat32/Any 的存在不证明 PG 数组保真；不排除其他载体，记为表示交接缺口。 |
| [datum_test.rs](../pkg/types/datum_test.rs)：`test_to_json`、`test_is_null`、`test_clone_datum` | 断言意图分别含 JSON 文本数组转换、标量 NULL、字节复制；没有据此证明 PG 空数组/两层 NULL、多维或下界。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`registeredFunctionClass::getFunction`、`invoke_factory` | 本文件未找到 array_length/cardinality/array_append 名称或核心工厂；json_array_append 是不同契约。常规链验参数数目后找工厂，已注册但缺工厂明确拒绝；未穷查动态扩展或其他 PG 路径，不能断言所有 SQL 必失败。 |
| [scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl` | `funcs/extensionFuncs → getFunction → getRetTp → ScalarFunction`，标准构造可折叠。这是应复用的规范标量链，未证明 PG 多态元素定型、数组返回身份或函数可见性接入。 |
| [pg_result.rs](../pkg/server/pg_result.rs)：`CatalogColumnType::wire_type`、`pg_type`、`value_binary`、`encode_formats` | 目录专用 code 映射若干数组 OID，binary 分支调用 binary_array；元数据按类型而非值生成，NULL 长度 -1。这些目录桥接不能推广成任意 SQL 数组的参数、形状或函数结果能力。 |
| [pg_types_test.rs](../pkg/server/pg_types_test.rs)：`common_type_oids_and_values`、`unsigned_binary_and_unsupported_types` | 前者仅断言普通标量 Q 结果；后者明确断言原生 JSON 即使为 NULL 也不得冒充 PG 目录数组。没有三函数、通用数组结果或扩展协议专项通过证据。 |

#### 范围函数契约

本块静态基线 `df3ca02b067f05466eec8eb01b020383e9f94fb9`（2026-10-23）；批次 1–8 的连续提交和五节正文均已核对，前置任务已删除。`pkg/types`、`pkg/expression`、`pkg/server` 无 doc.go。RustCodeGraph status/explore/query/定点 node 用于读取真实源码，图未提供名称字面量时以 rg 补充；以下只证明静态事实与断言意图。

| 入口/实际符号 | 已读事实与限制 |
| --- | --- |
| [datum.rs](../pkg/types/datum.rs)：`Datum`、Kind 常量、`Clone` / `Copy`；[datum_test.rs](../pkg/types/datum_test.rs)：`test_datum` | 已读 Kind 无通用 PG range 标签；盒含字节和 Any，未直接承载 range/subtype 身份、empty、无界与开闭标志。测试检查整数/字符串/字节及 Any 存取，不证明 range 保真。第 3 章未承诺通用 range 表示，这是显式前置缺口，不能用两端点元组、JSON 或 Any 存在来判支持。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`FunctionClassRegistry::from_specs`、`invoke_factory` | lower/upper 有单参数名称规格，但已读核心工厂表无这两个工厂或 range 工厂；本文件未找到 isempty/int4range/numrange 名称。名称不能证明 range 重载；动态工厂缺失会明确拒绝，未穷查其他初始化路径，不能断言全部 SQL 必失败。 |
| [scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`；[builtin_test.rs](../pkg/expression/builtin_test.rs)：`eval_builtin_func` | 正式构造取得类、调用 getFunction、读取 getRetTp 并包装 ScalarFunction，可按策略折叠；通用测试 helper 仍含迁移注释，不是 range 求值断言。应沿正式链接入已选签名，不新建 PG 求值器。 |
| [pg_result.rs](../pkg/server/pg_result.rs)：`pg_type`、`value_text`、`value_binary`；[pg_types_test.rs](../pkg/server/pg_types_test.rs)：`common_type_oids_and_values` | 已读映射为普通标量及目录专用桥接，未见 range OID 分支；文本透传不能证明 range 规范值或 binary codec。测试断言普通标量 Q 的 OID/值，没有本块 range 构造、边界、扩展协议专项证据。 |

#### 必需系统函数契约

本块静态基线 `f120e9541b21982d5033d704ea44d368f348eb00`（2026-10-23）；批次 1–9 提交及五节正文齐全，累计依赖满足。`pkg/expression/doc.go`、`pkg/server/doc.go` 均不存在。经 RustCodeGraph status、explore、定点 node 阅读真实源码；图未覆盖的名称字面量与模块挂载用 rg 补查。以下仅静态源码及测试断言意图，未运行测试。

| 实际入口/符号 | 已核实责任、客户端需求与限制 |
| --- | --- |
| [builtin_info.rs](../pkg/expression/builtin_info.rs)：`SessionInfo`、`database`、`connection_id`、`version`；[builtin_info_test.rs](../pkg/expression/builtin_info_test.rs)：`session_information_matches_go_values_and_missing_state_errors`、`expression_metadata_and_version_helpers_return_go_fields` | MySQL 局部 kernel：空 current_db→NULL，连接 u64 转 i64，版本取 MySQL ServerVersion；独立测试断言 test/空库、连接 1、版本非空。不是 PG 数据库身份、int4 backend PID 或 PG 版本返回身份的证据。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`registeredFunctionClass::getFunction` / `invoke_factory`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`；[lib.rs](../pkg/expression/lib.rs) | 名称查类→参数检查→工厂→RetType→ScalarFunction/可选折叠的共享链应复用；crate 挂载 info kernel 和独立测试。规格含 database/connection_id/version，已读核心工厂表没有这三项，动态注册未全面排除；没有证明五个 PG 候选走正式标量工厂。 |
| [pg_session.rs](../pkg/server/pg_session.rs)：`SessionQuery::parse` / `metadata`、`PgSession::schema` / `execute`；[pg_conn.rs](../pkg/server/pg_conn.rs) 的 Q 分支；[pg_extended.rs](../pkg/server/pg_extended.rs) 的 Parse/Execute 分支 | 专用语法只接受无 FROM 的单项 current_database()/current_schema() 及别名/pg_catalog 限定；Schema 读连接路径，Database 委托 SELECT DATABASE()；Q 与扩展查询调用 session.execute，扩展目录执行刷新 current_schema。此有界接线不是任意表达式或正式函数注册的替代品；启动 server_version 常量也不能证明 SQL version()。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)：`CatalogQuery::parse_session_with_types` / `expr_type` / `evaluate`；[pg_catalog_query_test.rs](../pkg/server/pg_catalog_query_test.rs)：`catalog_select_structure`、`catalog_binding_has_explicit_support` | 当前库来自上下文、schema 来自连接快照；current_database/current_catalog 限 pg_database，current_schema 限 pg_namespace，类型分支暂用 code 253。测试解析真实目录探测形状，并断言 current_database() FROM pg_locks 被 0A000 拒绝；这只是当前门禁，PG 目标允许普通表达式调用。 |
| [pg_query_test.rs](../pkg/server/pg_query_test.rs)：`pg_introspection_namespace_session_state`、`datagrip_catalog_live_metadata`、`namespace_catalog_live_metadata`；[pg_session_test.rs](../pkg/server/pg_session_test.rs)：`pg_introspection_namespace_parser_boundaries`；[pg_client_integration_test.rs](../pkg/server/pg_client_integration_test.rs) 的 libpq 目录探测脚本 | DataGrip 目录排序包含 current_database/current_schema，libpq 脚本包含 current_schema 探测，故这两项是客户端证据最强的首批；独立测试还有两连接、空路径 NULL、RESET、Parse 后 SET 再 Execute 的断言意图。version 为握手/能力探测次序候选，现有 server_version 断言只覆盖启动参数；pg_backend_pid/pg_typeof 未找到同等客户端需求断言，列为身份与类型一致性候选，不虚构需求频率或运行通过。 |

#### MySQL专属函数隔离

静态基线 `e45ddcd620396699286faabecbc8f53e2525ca1d`（2026-10-23）；批次 1–10 的连续提交和五节累计正文已核对，依赖满足。`pkg/expression/doc.go` 不存在；使用 RustCodeGraph status/explore/定点 node，名称字面量用 rg 补查。以下是源码及独立断言意图，均未运行。

| 实际入口/符号 | 已核实内容及证据边界 |
| --- | --- |
| [builtin_control.rs](../pkg/expression/builtin_control.rs)：`eval_if_null`；[builtin_control_test.rs](../pkg/expression/builtin_control_test.rs)：`if_and_ifnull_cover_truth_conversion_short_circuit_and_null` | 局部测试断言非空首参跳过 Error、NULL 回退及全 NULL；正式链另由 builtin.rs 的 ifnull_factory、CoreBuiltinKind::IfNull 和 InferType4ControlFuncs 构造/分型求值，不把局部 SqlValue helper 冒称实际工厂调用。 |
| [builtin_time.rs](../pkg/expression/builtin_time.rs)：`date_format`；[builtin_time_test.rs](../pkg/expression/builtin_time_test.rs)：`test_date_format`；[builtin_time.go](../pkg/expression/builtin_time.go)：`dateFormatFunctionClass.getFunction` / `builtinDateFormatSig.evalString` | Rust helper 接受非 NULL NaiveDateTime 与掩码；测试入口委托通用套件，DATE_FORMAT 专项 NULL/模板断言仍为注释。Go 真实工厂按 datetime/string 转换、推导模板长度，求值保留 NULL 与非法/零日期的 sql_mode 处理；helper 不能证明全部已移植。 |
| [builtin_json.rs](../pkg/expression/builtin_json.rs)：`json_extract`；[builtin_json_test.rs](../pkg/expression/builtin_json_test.rs)：`test_json_extract`；[builtin_json.go](../pkg/expression/builtin_json.go)：`jsonExtractFunctionClass` / `builtinJSONExtractSig.evalJSON` | helper 走 parsed_paths→BinaryJSON.Extract；独立测试断言多路径 [1,2]、缺失 None、非法路径 Err。Go 工厂验证类型并对文档/路径分别转换；Rust 正式 JsonExtract 分支同样逐参求值、解析路径、Extract，SQL NULL 与 JSON null 不能混为一谈。测试不证明协议或双方言门禁。 |
| [builtin_info.rs](../pkg/expression/builtin_info.rs)：`tidb_version`；[builtin_info_test.rs](../pkg/expression/builtin_info_test.rs)：`expression_metadata_and_version_helpers_return_go_fields`；[builtin_info.go](../pkg/expression/builtin_info.go)：`tidbVersionFunctionClass.getFunction` / `builtinTiDBVersionSig.evalString` | 实际存在的 TiDB 管理/构建内省函数 TIDB_VERSION()，Rust 与 Go 都取 printer.GetTiDBInfo；独立断言仅非空。Go 为 ETString、flen 按信息长度、返回非 NULL；不把 PG version() 或握手版本代入该值。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`invoke_factory`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl` | 四名称规格为 2/2、2/2、2/不限、0/0；已读核心表包含 ifnull/json_extract，未见 date_format/tidb_version，动态注册未全面排除。正式调用仍为名称查类→参数检查→工厂→getRetTp→ScalarFunction/折叠；缺工厂明确报 implementation is not linked。存在 helper/规格不证明 SQL 可用，更不证明 PG 隐藏。 |

#### 运算符调用契约

本块基线 `66cac76435e4b8313f42e18ed8a60e3301ee6685`（2026-10-23）；已核对批次 1–11 的连续提交和对应五节正文，最近交付为 MySQL 专属函数隔离。`pkg/expression/doc.go`、`pkg/parser/doc.go` 与 `pkg/server/doc.go` 不存在。使用 RustCodeGraph status、explore、query、定点 node 读取真实源码，图未定位名称字面量时用 rg 补充；以下全为静态事实及断言意图，未执行测试。

| 真实入口/符号 | 责任与限制 |
| --- | --- |
| [builtin_arithmetic.rs](../pkg/expression/builtin_arithmetic.rs)：`ArithmeticExpr::build` / `eval`、`ArithmeticOp`、`Signature`、`EvalContext` | 局部 owned Expression 内核将 Divide 分到 Real/Decimal，将 IntDivide 分到整数/Decimal 整除；NULL 传播，除零按上下文报错或警告。不能因 IntDivide 算法存在便声称 PG `/` 的 int4 签名已接入。 |
| [builtin_compare.rs](../pkg/expression/builtin_compare.rs)：`compare_datums`、`Op::NullEq`；[builtin_op.rs](../pkg/expression/builtin_op.rs)：`logical_or`、`unary_minus_int` | 局部比较先处理 Error，再处理 NULL：NullEq 两空为 true、一空为 false，普通比较为 NULL；一元负号检查 signed/unsigned 溢出。MySQL 真值规则与 i64 容器不证明 PG boolean 或 int4 身份。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`CORE_BUILTIN_FACTORIES`、`invoke_factory`、`CoreBuiltinKind::Arithmetic` / `Compare`、`eval_boolean` / `evalReal`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl` | 正式链为名称表 → getFunction → 工厂 → RetType → ScalarFunction 求值；div、intdiv、unaryminus、nulleq 有核心工厂。正式 div 选 Real 或 Decimal，nulleq 求值处理两空；已读正式 REAL 除零直接返回 NULL，不等同局部内核警告策略。两条路径分别取证，不能把局部断言当正式链或 PG resolver 证据。 |
| [lib.rs](../pkg/expression/lib.rs)、[builtin_arithmetic_test.rs](../pkg/expression/builtin_arithmetic_test.rs)、[builtin_arithmetic_2_aster_unit_test.rs](../pkg/expression/builtin_arithmetic_2_aster_unit_test.rs) | crate 挂载算术内核及独立测试入口，入口再挂 go_parity；`divide_preserves_division_by_zero_policy_overflow_and_decimal_scale` 断言除零 NULL/警告、错误模式和 1/8 的 scale 4，`integer_division_handles_four_signedness_pairs_and_minimum_overflow` 断言 -7 DIV 2 为 -3 和 i64 最小值除 -1 溢出；均非 PG SQL 证据。 |
| [builtin_op_test.rs](../pkg/expression/builtin_op_test.rs)、[builtin_compare_test.rs](../pkg/expression/builtin_compare_test.rs)、[builtin_compare_7_aster_unit_test.rs](../pkg/expression/builtin_compare_7_aster_unit_test.rs) | 前者 `unary_minus_covers_signed_unsigned_decimal_and_overflow`、`logical_operators_cover_mysql_three_valued_logic_and_short_circuit` 覆盖局部负号/逻辑；比较入口挂载的 `all_comparison_operators_match_null_and_unsigned_semantics` 覆盖两空、一空、等值和 unsigned 比较。未证明 PG IS NOT DISTINCT FROM 分析/求值或 wire 返回 boolean。 |
| [lexer.rs](../pkg/parser/lexer.rs)：`HasPipesAsConcatMode` 分支 | 未启用 PIPES_AS_CONCAT 时将 pipes token 转成 pipesAsOr；说明 MySQL `\|\|` 受 sql_mode 影响，不能从内部 concat 名称推断两入口同义。PG 语法接线和优先级归第 2 章，本块不修改解析器。 |

#### 普通聚合调用契约

静态基线 `9ee00a13fb92aae94dc0dbc4fa9cce168dc03f99`（2026-10-23）；批次 1–12 的连续提交及五节累计正文已核对，前置文档依赖满足。`pkg/expression/doc.go`、`pkg/expression/aggregation/doc.go`、`pkg/executor/doc.go` 均不存在。使用 RustCodeGraph status/explore/query/定点 node 阅读源码，模块挂载和图未表达的字面量用 rg 补查。下表仅为源码及独立测试断言意图，未执行测试。

| 实际入口/符号 | 已读责任与证据边界 |
| --- | --- |
| [descriptor.rs](../pkg/expression/aggregation/descriptor.rs)：`NewAggFuncDesc`、`AggFuncDesc::GetAggFunc` / `Split`；[base_func.rs](../pkg/expression/aggregation/base_func.rs)：`TypeInfer`、`typeInfer4Count` / `typeInfer4Sum` / `typeInfer4Avg` | 描述符含 Args/RetTp、Mode、HasDistinct、OrderByItems，未见 FILTER 字段；构造先推断类型，GetAggFunc 按名称构造 count/sum/avg，Split 为 AVG 生成 count/sum 两列的最终输入。现有类型规则为 MySQL COUNT longlong、整数 SUM/AVG decimal、浮点 double，不能证明 PG 重载或 FILTER 已接线。 |
| [aggregation.rs](../pkg/expression/aggregation/aggregation.rs)：`Aggregation`、`AggEvaluateContext`、`aggFunction::CreateContext`；[count.rs](../pkg/expression/aggregation/count.rs)：`countFunction::Update` / `GetResult`；[sum.rs](../pkg/expression/aggregation/sum.rs)：`sumFunction`；[avg.rs](../pkg/expression/aggregation/avg.rs)：`avgFunction::Update` / `GetPartialResult` / `GetResult` | 每组 context 初值 Count=0、Value=NULL，可带 DISTINCT checker；COUNT 跳过 NULL、最终阶段累加局部计数；SUM 委托 updateSum，AVG 保存并合并 count/sum，decimal 最终化使用 MySQL div_precision_increment 与 scale。不是普通标量工厂逐行求值的替代形式，也不是 PG 精度/错误证据。 |
| [builder.rs](../pkg/executor/builder.rs)：`BuildTypedPhysicalPlan`、`build_typed_physical_plan`；[typed_hash_agg.rs](../pkg/executor/typed_hash_agg.rs)：`TypedHashAgg::new` / `prepare` / `update_row`、`update_aggregate` / `finish_aggregate` | PhysicalHashAgg 传 AggFuncs、GroupByItems、context 给 TypedHashAgg；它另持每组 AggregateState，按编码 key 分组及去重，空输入仅无 GROUP BY 时补一个组；RetTp 生成 schema。已读更新路径未消费 OrderByItems，也未见逐聚合 FILTER；COUNT 使用 wrapping_add。此为可复用执行链入口，不证明 PG 分组相等、排序、溢出或两条聚合路径等价。 |
| [count_test.rs](../pkg/expression/aggregation/count_test.rs)：`dedup_mode_does_not_produce_a_count`；[avg_test.rs](../pkg/expression/aggregation/avg_test.rs)：`zero_count_float_partial_state_matches_go_ieee_division` / `zero_count_decimal_partial_state_matches_go_logged_error_result` | COUNT 仅断言 DedupMode 不计数；AVG 人工构造非 NULL sum/零 count，分别预期 infinity/非 NULL 零 decimal，不能用来证明正常空输入的 PG NULL 行为。独立测试由 aggregation/lib.rs 挂载，均未运行。 |
| [typed_hash_agg_test.rs](../pkg/executor/typed_hash_agg_test.rs)：`typed_hash_agg_groups_across_input_pages_and_pages_output`、`typed_hash_agg_scalar_empty_input_emits_count_zero` | 经 BuildTypedPhysicalPlan 构造真实 scan/HashAgg，断言跨输入页两组 count 为 2/1、空全局输入一行 count=0；executor/lib.rs 挂载。未覆盖本块 PG FILTER、排序、numeric 元数据或协议，不能据此升级 SQL 支持。 |

#### 窗口函数调用契约

静态基线 `b2dccdbd5272b1f34baf75a7d002e6e1868a240f`（2026-10-23）；批次 1–13 连续提交及累计五节正文已核对，前置文档依赖满足。RustCodeGraph status/explore/query/定点 node 用于源码导航，模块挂载用 rg 补查；`pkg/expression/doc.go`、`pkg/expression/aggregation/doc.go`、`pkg/executor/doc.go`、`pkg/executor/windows/doc.go`、`pkg/executor/aggfuncs/doc.go` 均不存在。以下是静态源码与断言意图，未执行测试。

| 实际入口/符号 | 责任与证据限制 |
| --- | --- |
| [window_func.rs](../pkg/expression/aggregation/window_func.rs)：`NewWindowFuncDesc`、`NeedFrame`、`UseDefaultFrame` | 描述符内嵌 baseFuncDesc，按 MySQL 字段规则推断返回/可空性；LEAD/LAG 检查非 NULL 无符号常量 offset。NO_FRAME 列出排名/偏移函数，row_number 的内部 CURRENT ROW 帧是优化约定，不能当作 PG SQL 默认帧。描述符不承载完整分区/排序/帧语义。 |
| [builder.rs](../pkg/executor/aggfuncs/builder.rs)：`build_window_function`、`build_lead_lag` | 选择 Rank/RowNumber/LastValue/Lead/Lag 实现描述，offset/default 读取常量；其存在不证明 PG 每行动态 offset/default、多态签名或物理计划桥接。 |
| [windows/builder.rs](../pkg/executor/windows/builder.rs)：`PhysicalWindowPlan`、`build`；[window.rs](../pkg/executor/windows/window.rs)：`FrameType`、`FrameBound`、`WindowFunction`、`RowNumber`、`Lag` | 物理计划接收预建 WindowFunction、分区/排序列及可选帧，选择缓冲/流水线路径；None 是整分区，不负责解析 PG 默认帧。帧枚举只有 Rows/Range，边界 num 为 u64；RANGE offset 经 f64 数值比较，OrderBy 仅有列号/降序、sql_cmp 将 NULL 排最小，无完整 PG collation/NULLS 配置证据。RowNumber 返回 UInt，Lag 接收 usize offset/固定 default；不能据容器推断 PG int8 类型或动态 offset 可用。 |
| [window_func_test.rs](../pkg/expression/aggregation/window_func_test.rs)：`window_pushdown_honors_tiflash_specific_scalar_blacklist` | 只断言 TiFlash scalar 黑名单拒绝下推，不是帧语义测试；由 aggregation/lib.rs 挂载。 |
| [window_sql_test.rs](../pkg/executor/windows/window_sql_test.rs)：`go_test_window_functions_covers_partition_rows_and_range_frames`、`go_test_window_functions_data_reference_preserves_lag_across_chunks`、`go_test_sliding_window_functions_covers_asymmetric_and_empty_frames`、`go_test_issue_45964_and_46050_preserves_empty_frame_nullability` | 直接 build/run_executor，断言分区重置、ROWS/RANGE 升降序、跨块 LAG、空帧 SUM=NULL/COUNT=0/行号递增；windows/lib.rs 挂载。文件名含 sql 不表示解析或协议测试，未提供 PG rank/lead/last_value、GROUPS、默认帧或 wire 实测证据。 |

#### 有序集合聚合契约

静态基线 `e8670bc2ef6e20b03deef6d3cb825e33e42a412d`（2026-10-23）；批次 1–14 连续提交及对应五节正文已核对，累计前置文档依赖满足。使用 RustCodeGraph status/explore/定点 node 阅读真实入口，rg 补查模块挂载；`pkg/expression/aggregation/doc.go`、`pkg/executor/doc.go`、`pkg/executor/aggfuncs/doc.go` 均不存在。以下只有源码及独立断言意图，没有执行测试。

| 实际入口/符号 | 责任与证据限制 |
| --- | --- |
| [descriptor.rs](../pkg/expression/aggregation/descriptor.rs)：`AggFuncDesc`、`NewAggFuncDesc`、`GetAggFunc` | Args/RetTp、Mode、HasDistinct、OrderByItems 为普通聚合描述；构造初始化空排序列表，未见区分 direct/aggregated arguments 的字段。GetAggFunc 按名称分派，未知名 panic；不能直接用其普通参数列表证明 PG WITHIN GROUP 接线。 |
| [builder.rs](../pkg/executor/builder.rs)：`build_typed_physical_plan`；[typed_hash_agg.rs](../pkg/executor/typed_hash_agg.rs)：`update_row`、`evaluated_arguments`、`update_aggregate`、`finish_aggregate` | PhysicalHashAgg 将 AggFuncs/GroupByItems 交 TypedHashAgg，逐行计算 Args 后更新每组状态，最后输出结果。已读链没有 direct 参数的每组求值、WITHIN GROUP 排序输入或 percentile 插值分支，不据普通聚合复用点宣称支持。 |
| [aggfuncs/builder.rs](../pkg/executor/aggfuncs/builder.rs)：`build`、`build_approx_percentile`；[func_percentile_test.rs](../pkg/executor/aggfuncs/func_percentile_test.rs)：`percentile_selects_one_based_ordinal_and_ignores_nulls`、`percentile_matches_go_boundaries_memory_and_reset_lifecycle` | 工厂选择 ApproxPercentile，第二参数取 i32 常量百分数；独立测试用 50 选中位样本、忽略 NULL、合并清空源，0 的 result 为 None。不是 PG 0–1 比例、连续插值或比例 0 取首值的证据，不批准直接别名。 |
| [window_func_test.rs](../pkg/executor/aggfuncs/window_func_test.rs)：`test_window_functions`；[typed_hash_agg_test.rs](../pkg/executor/typed_hash_agg_test.rs)：`typed_hash_agg_honors_cancellation_before_consuming_child` | 前者直接断言 CUME_DIST、PERCENT_RANK、RANK 等窗口状态；后者经 BuildTypedPhysicalPlan 检查取消错误。分别由 aggfuncs/lib.rs、executor/lib.rs 挂载。两者均非有序集合聚合 SQL/协议证据；排名窗口不能替代 percentile 的聚合模型。 |

#### 集合返回函数契约

静态基线 `ab189d0855f92c0fa88a9960407bd2f155eb7563`（2026-10-23）；已核对批次 1–15 的连续文档提交及五节对应正文，累计前置依赖满足。通过 RustCodeGraph status/explore/query/定点 node 读取以下入口，模块挂载由 rg 补查；expression、executor、server 包均无 doc.go。源码和独立测试只证明静态接线及断言意图，本轮未执行测试。

| 实际入口/符号 | 责任与证据限制 |
| --- | --- |
| [builtin.rs](../pkg/expression/builtin.rs)：`registeredFunctionClass::getFunction`、`invoke_factory` | 验证参数数目后返回单个 builtinFunc；沿前块标量链复用名称/类型能力，不据此宣称存在 SRF 的行生成接口。图查询 generate_series 无符号命中仅是检索结果，不是全仓不支持的证明。 |
| [physical_plan_runtime.rs](../pkg/executor/physical_plan_runtime.rs)：`projection_columns`、`project_row`、`ExecutePhysicalPlan` | 已读投影只接受解析后的列索引，project_row 返回一个 Row，不能承载每调用零至多行、关联重启或多个 SRF 的协调耗尽；执行入口不是通用 SRF 接线证据。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)：`CatalogQuery::project`、`reject_projection_unnest`、`unnest_projection` | 目录专用投影与 ARRAY 子查询有受限处理，后者仅接受 unnest 列及其 cast，嵌入普通表达式的 unnest 有明确 0A000 拒绝。此处是目录边界，不能推广到用户表 FROM/LATERAL 或普通 SELECT。 |
| [pg_catalog_query_test.rs](../pkg/server/pg_catalog_query_test.rs)：`catalog_datagrip_scalar_and_lateral_bounds`；[pg_query_test.rs](../pkg/server/pg_query_test.rs)：`simple_query_roundtrip` | 前者断言 DataGrip 目录函数关系的 ordinality/列别名，并预期目录 CROSS JOIN generate_series(1,2) 报 0A000；后者断言 SELECT 1 的 TDCZ、值、OID 20、文本格式及 NULL/空串区别。均由 server/lib.rs 挂载；前者不是通用 SRF 通过，后者不是 int4 SRF 元数据证据。 |

#### 章节验收与后续交接

本块收口蓝图任务 4 的步骤 1–4 与交付要求，不新增函数实例或实现。批次 1–16 的提交依次为 `e4915aee92`、`83aea884a4`、`e128b200ac`、`6e15d36d64`、`af99cc260d`、`a485e8e82c`、`2168dce847`、`df3ca02b06`、`f120e9541b`、`e45ddcd620`、`66cac76435`、`9ee00a13fb`、`b2dccdbd52`、`e8670bc2ef`、`ab189d0855`、`430cbb8339`；每块在五个全局小节中均有正文，累计文档依赖满足。来源蓝图任务仍保留未开始，本次不替它变更状态。

本块静态基线为 `430cbb8339`（2026-10-23）。`pkg/expression/doc.go`、`pkg/server/doc.go` 均不存在；通过 RustCodeGraph status/explore/定点 node 复核以下真实入口和独立测试，全部只读、未运行。

| 入口与符号 | 本次复核事实及限制 |
| --- | --- |
| [builtin.rs](../pkg/expression/builtin.rs)：`FunctionClassRegistry::from_specs/get`、`invoke_factory`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`、`NewFunctionBase` | 名称字符串映射类、规格初始化核心工厂、动态工厂缺失明确报错；正式构造保留工厂返回类型且有独立折叠策略。优先沿此链接线，不能由名称存在推出 PG 签名选择/方言过滤已实现，也不能未排除动态注册便宣称某函数必失败。 |
| [infer_pushdown.rs](../pkg/expression/infer_pushdown.rs)：`can_func_be_pushed`、`can_expr_push_down`；[infer_pushdown_test.rs](../pkg/expression/infer_pushdown_test.rs)：`tiflash_rejects_signatures_not_explicitly_supported_by_go` | 当前检查存储能力、名称/签名黑名单、返回类型、PbCode、子树和元数据可序列化；独立测试负例含 RoundFuture/TruncateFuture/LeastDecimal/GreatestDecimal。这是既有门禁与断言意图，不是 PG 跨方言语义等价审批。 |
| [pg_result.rs](../pkg/server/pg_result.rs)：`pg_type`、`value_text/value_binary`、`encode_formats` | 从列/NativeType 推导 OID，当前 typmod 固定 -1，NULL 长度 -1，按 portal 格式编码；完整编码后才交付，不能从第一行值反推类型，也不证明各函数能产出正确 native 身份。 |
| [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)：`parse_bind_execute_sync` | 实际断言覆盖目录 Q/Describe/Execute、零行描述、显式 OID 重复参数、绑定 NULL 及 Sync 恢复；普通省略 OID 参数负例预期 0A000。不能据此认定本章 PG 重载或函数结果 text/binary 已通过。 |
| [pg_client_integration_test.rs](../pkg/server/pg_client_integration_test.rs)：`pg_introspection_clients` → `run_clients` | 同一服务启动 MySQL/可选 PG 入口、运行 libpq/JDBC 工作流，再断言 MySQL 查询与 COM_PING；证明既有测试设计关注目录客户端和共存。源码中外部客户端成功断言不等于本轮运行记录，更不覆盖本章所有签名。 |

### 目标行为

#### 三层身份与契约模板

三层分别回答“谁计算”“SQL 调用选哪个契约”“客户端看到何种类型”。规范实现 ID 是文档中的稳定算法/语义身份，需附适用输入域和版本；不是 SQL 名称、Rust 类型、TiPB 枚举值、PG 函数对象 OID 或返回类型 OID，也不预设新增 Rust API。同一算法可承接多个已证等价的方言签名；同名不等价签名必须独立分流，同值不等于同型。第 1 章的能力 ID 标识可验收契约，与实现 ID 分别追踪。

| 层 | 标量候选贯通示例（目标，当前未验证） | 选择/拥有者与禁止推导 |
| --- | --- | --- |
| 规范实现 | 文档候选 `scalar.abs.signed.checked/v1`：已定型有符号整数的绝对值，按声明域检查结果；`abs_int` 仅为可复用的局部 i64 算法候选。 | 共享 expression 求值拥有算法；PG int4/int8 可候选共享值操作，但各自域检查和结果身份不可省略，尚未证明可直接复用。 |
| 方言签名 | `FUNC-ID-ABS/int4/pg`：可见内建 `pg_catalog.abs(int4) → int4`；关联 `FUNC-ID-ABS/int8/pg` 与 `FUNC-ID-ABS/signed/mysql`，后者 SQL 名称 ABS、按 MySQL Int 规则返回 BIGINT。这些为待新增能力记录。 | 由所属连接方言及第 2 章名称环境筛选，再按参数类型和第 3 章隐式 cast 资格选择；不能仅按 `abs` 和参数数目选择。函数重载不由期望返回 OID 或 Bind 的值决定，完整候选排序交下一块。 |
| 协议返回类型 | PG int4 结果为 OID 23、typlen 4、typmod -1；int8 为 20/8/-1。MySQL signed 结果走自身 BIGINT code 8/unsigned 标志未设置，其他长度/flags/decimals 需补参考证据。 | 第 3 章保留规范身份，server 分别生成 PG/MySQL 元数据及载荷。23/20 是结果类型 OID，不是 abs 函数对象 OID；返回 1、SQL NULL 或零行不能决定类型。 |

下表既是本章统一模板，也是上述 signed ABS 候选的首个实例。后续每个实例必须填写“方言目标、证据来源、当前缺口”三项；不适用写原因，未知区分“待定义（目标缺口）”与“未验证（当前运行缺口）”，不留空、不用裸 null。公共状态与用例关联沿第 1 章，不建立第二套证据格式。

| 必填字段 | signed ABS 候选填写、依据与缺口 |
| --- | --- |
| 关联、分类与调用模型 | 关联上表能力 ID、规范实现候选 ID、场景 ID 与基线 SHA；标量。正整数/域内绝对值是共享语义候选，开放等价范围前仍须双方言分别验收；不推广到全部数值族。来源为本块源码与第 1 章证据规则，当前运行未验证。 |
| 名称/别名与方言可见性 | PG 的 abs 及 pg_catalog 限定名，MySQL 的 ABS 及自身大小写规则分别记录；没有批准额外跨方言别名。PG 限定名、search_path 与重载规则依据官方 §10.3、第 2 章；当前两入口可见性及函数名称接线未验证。 |
| 重载与隐式 cast | 首例只接受已定型 int4/int8 的精确 PG 签名；MySQL signed 按原 Int 分支。unknown 字面量、未定型 NULL 与参数 OID 0 分开记录；候选转换只能消费第 3 章 I 资格，不能借结果 wire 扩宽放开 SQL 转换。完整歧义/候选排序本块待定义，交注册/重载块，不能套 UNION 共同类型算法。 |
| NULL 与求值条件 | PG 两个重载为 strict：合法 typed NULL 返回同型 SQL NULL，不调用非 NULL 算法；无匹配签名不能由 NULL 绕过类型检查。依据 REL_18_0 的 pg_proc.dat 与 pg_proc.h 默认值。MySQL NULL 传播目标由 Go TestAbs 的 nil 断言支持；Rust 全链 typed NULL 仍未验证。 |
| collation、时区与会话前提 | 对已定型整数的绝对值运算不适用 collation/时区（纯整数，不比较文本或处理时间）；转换或名称查找若依赖会话则另外列明，不能从算法无依赖推广为 resolver 无依赖。依据整数算法和第 2/3 章，跨连接隔离未验证。 |
| 确定性、折叠与副作用 | PG 首例目标 immutable、无数据副作用，依据 pg_proc 默认属性及 int4abs；常量输入可在规划期求值报错，不能保证错误只在 Execute。MySQL 优化属性继续沿原契约；当前折叠、缓存及方言隔离未验证。其他函数须分别记录 stable/volatile 和状态依赖，不从同名或缓存策略猜测。 |
| 返回类型、值域与修饰符 | PG int4→int4、int8→int8，结果域仍为各自 signed 域，负最小值取绝对值拒绝，NULL 不改型；MySQL Int 保持 BIGINT/unsigned 与 precision/scale 原规则。PG 依据 §9.3、REL_18_0 pg_proc.dat 与 int.c；MySQL 依据 Go 工厂。Rust PG 结果宽度保真未验证。 |
| wire 元数据与格式 | PG 23/4/-1、20/8/-1，列名由 AS v 固定；表达式来源表/属性号为 0，格式依 Q/statement Describe/portal，NULL 无载荷且长度 -1。PG RowDescription 不含可空性位，逻辑可空性单独记，不能发明 wire 字段。MySQL 独立核对 code/flags/charset/length/decimals 及 NULL；除上表 code/unsigned 目标外其精确字段待定义，需对应客户端参考记录。当前两入口均未采集。 |
| 下推与执行位置 | 默认不批准跨方言下推；须逐签名证明类型、NULL、值域/错误、collation/时区和结果元数据等价后才能批准。TiPB 签名存在、注册名存在或局部 abs_int 可用均不足；当前 PG ABS 下推等价证据未知、无性能结论。 |
| 错误字段与失败阶段 | PG int4 负最小值目标 22003；零参数 abs() 无匹配签名目标 42883。成功时 SQLSTATE/错误字段不适用。错误记录 S/V/C/M 与适用 D/H/P 的实际存在性、原 SQL 字符位置及分析/折叠/求值阶段；P 和本地映射未验证。MySQL 错误码、SQLSTATE、消息/警告按自身契约采集，不直接复用 PG 字段。依据 int.c、附录 A、§54.8；本块不规定未取证的完整消息文本或位置数值。 |
| 验收与证据状态 | 使用下方 FUNC-ID 用例族，关联源码、独立测试、官方来源及运行记录；静态源码/断言意图已定位，AsterSQL/PG/MySQL 实测均未采集。实现者须补实际构建、精确参考版本、完整命令/退出码、有效断言数量、原始输出；零用例不能升级支持。 |

官方来源（2026-10-23 查阅，版本 PostgreSQL **18**）：[§9.3 数学函数](https://www.postgresql.org/docs/18/functions-math.html)支持 ABS 同族返回；[§10.3 函数类型解析](https://www.postgresql.org/docs/18/typeconv-func.html)支持名称/参数候选、精确匹配及隐式转换筛选；[§52.39 pg_proc](https://www.postgresql.org/docs/18/catalog-pg-proc.html)定义 strict/volatility。精确 int4/int8 签名和 strict/immutable 默认属性核对官方固定 tag **REL_18_0** 的 [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat) 与 [pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h)，结果 OID/typlen 核对同 tag [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)，int4 最小值溢出核对 [int.c::int4abs](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int.c)。[§54.7 消息格式](https://www.postgresql.org/docs/18/protocol-message-formats.html)支持类型/格式/NULL 字段，[§54.8 错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)与[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持诊断记录。以上定义目标；18.6 仍仅为来源指定基线，无该补丁版本运行证据，不将 18 大版本手册或 18.0 静态源码记为 18.6 差分通过。

#### 注册工厂与方言可见性

沿用“三层身份与契约模板”，本块只定义可见候选到正式工厂的责任顺序。所属连接方言和第 2 章名称环境必须先限制可见集合，PG 再依 schema/search_path、名称与参数候选选择签名，类型转换只消费第 3 章资格；不能先在进程全局表命中另一方言实现后再靠结果 OID 修补。限定名称不能绕过方言边界；同一实现可被双方言签名引用，但别名须有逐签名等价证据。`extensionFuncs` 是共享内核的扩展注册入口，不自动授权 PG 扩展加载、目录对象或跨方言公开。

| 阶段 | 必须取得的证据；未满足时的边界 |
| --- | --- |
| 名称存在且可见 | 连接方言、原名称/限定名、名称环境和候选签名的记录；注册表存在只说明内部名称存在，隐藏名称不得泄露到 SQL 可见列表或落入另一方言候选。 |
| 参数合法 | 可见签名的参数数目、声明类型和可用隐式 cast 证据；参数数目合法不等于重载唯一，NULL 也不绕过签名选择。完整排序、unknown/多态/可变参数和歧义由下一块细化。 |
| 工厂接线 | 真实 crate 挂载、初始化顺序、类到具体工厂及返回类型传播；不能把注册列表、TiPB 签名或 `.rs.md` 当接线证据。已选签名缺工厂为实现缺口，不能改选另一方言或冒充未知名。 |
| 可求值及可交付 | 正式构造和求值的值/NULL/错误断言，再加双方言 SQL/协议类型与元数据证据；局部算法或工厂成功不覆盖 SQL 入口。缓存、折叠和跨连接复用必须保留选定方言签名，不能以值或全局当前方言选实现。 |

本块四个用例族引用首块全部必填字段，差异和缺口如下；均为待新增能力/用例，不是当前支持声明。

| 模板字段 | 本块填写与限制 |
| --- | --- |
| 关联、分类、调用模型、实现 ID | 下方 FUNC-REG 四族均为标量名称/工厂门禁契约；date_trunc 为 PG 专属，IFNULL 为 MySQL 专属，UNKNOWN 为拒绝边界，UNLINKED 为工程缺口夹具。ABS 继承首块候选 ID；date_trunc/IFNULL 具体规范实现 ID 待函数族块定义，不以 SQL 名称代替。 |
| 名称、别名、可见性、重载、cast | 以验收表明确的签名/方言为限，不批准 DATE_TRUNC 的 MySQL 别名、IFNULL 的 PG 别名或 COALESCE 改写；使用内建限定名验证 PG，不新增 resolver。测试库不含同名用户/扩展函数；精确重载、显式已定型参数优先，其他 cast/排序由下一块给证据。 |
| NULL 与求值条件 | IFNULL 的一空取后值、全空返回同型 NULL；date_trunc typed NULL 的 strict 目标待函数族官方属性补证，本块不以此作为已定义成功断言。UNKNOWN/UNLINKED 无成功值或 NULL，NULL 参数不消除门禁。ABS NULL 沿首块；所有当前全链行为未验证。 |
| collation、时区、会话 | 固定 UTF-8、PG search_path 含 pg_catalog、无同名用户函数；MySQL 选定测试库，记录 sql_mode、charset/collation。本例 timestamp without time zone 按字面时间截断，不依 TimeZone 转换；文本 IFNULL 沿 MySQL collation，精确派生规则待对应函数族取证。 |
| 确定性、折叠、副作用、下推 | 本块不新增 volatility/折叠批准，具体优化属性待函数族块；纯常量可能在规划折叠，门禁必须先于使用隐藏实现。四族无数据副作用，UNLINKED 构造拒绝；不批准跨方言下推，工厂存在不提供等价证据，无性能结论。 |
| 返回类型、wire、错误、证据 | 见下表，PG timestamp 的 OID/typlen 为 1114/8，typmod 目标 -1；MySQL 字符串精确 code/flags/charset/length/decimals 待参考采集，不能猜测。成功列名 v、表达式来源表/属性号为 0；拒绝没有成功列/载荷。PG 错误沿首块 S/V/C/M 与适用 D/H/P，精确消息/P/失败阶段和 MySQL 诊断仍待采集；各族当前为静态取证与文档目标，运行未验证。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§10.3 函数解析](https://www.postgresql.org/docs/18/typeconv-func.html)定义可见候选与精确/隐式转换匹配；[§9.9 日期时间函数](https://www.postgresql.org/docs/18/functions-datetime.html#FUNCTIONS-DATETIME-TRUNC)定义 date_trunc(text,timestamp) 的返回类型与截断行为；[§9.18 条件表达式](https://www.postgresql.org/docs/18/functions-conditional.html#FUNCTIONS-COALESCE-NVL-IFNULL)将 IFNULL 列为其他数据库的类似能力，并说明 COALESCE 并非普通函数，本块据此不把它作为开放 PG IFNULL 别名的依据。timestamp 的 OID/长度由官方 **REL_18_0** [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)核对；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持 42883/0A000 分类。另一入口隐藏名称是 AsterSQL 的目标边界，不能只从 PG 手册推断 MySQL 支持集合。18 大版本手册和固定 18.0 源码均未提供本轮 18.6 运行差分证据。

#### 重载与隐式转换选择

继承首块模板及[显式隐式转换契约](#显式隐式转换契约)，只定义普通函数签名选择。先由前块过滤方言、名称环境与可见候选，再根据实参数量和声明输入类型选择；返回类型、期望 wire OID、参数值及注册顺序均不用于打破歧义。函数式类型转换的单参数特殊语法归第 2/3 章，不把它当普通重载。

候选阶段消费第 3 章 I 资格：精确签名优先；没有精确签名时剔除不能隐式转换的候选，依次比较精确位置数、需转换位置的类别优选类型。unknown 不构成精确匹配；仍有多个候选时按各 unknown 位置的 string 类偏好、统一类别及类别优选类型筛选；已知参数全为同型时再尝试用该型约束 unknown，仍不唯一则拒绝。不能把 UNION/CASE 共同类型推导直接套在函数候选上，不能以全局“最低 cast 成本”代替官方顺序。domain、多态和用户类型未开放时保留能力门禁，不从基础类型规则推导已支持。

unknown 字符串、未定型 NULL 和未指定 OID 的参数分别保留来源。typed NULL 带已知类型参加同一筛选，合法 strict 签名才传播同型 NULL，非法类型不因空值放行。参数在 Parse/Prepare 分析时定型，Bind 只解码既定类型；换值、NULL、文本/二进制格式不得重新选签名。候选选择之后才插入合法 coercion、执行输入解析和域检查；例如已选数值签名后文本内容非法是转换错误，不能失败后试另一个重载。折叠可提前触发值错误，不承诺都延后 Execute。

可变参数先按调用实参数量将声明数组参数展开为元素签名；显式 VARIADIC 保留数组调用、不做该展开。同 schema 展开后同输入签名的非可变函数优先，跨 schema 同签名按路径；不同输入签名不能只因路径靠前获胜。默认参数允许省略可默认位置，但同 schema 同剩余输入签名仍可能歧义。这些是候选规则，不授权本轮实现 UDF、默认参数、任意 schema 或多态；缺所需载体时标实现缺口，不能按名称和 min/max 参数数目冒充完成。

本块五组实例统一引用“三层身份与契约模板”，字段补充如下：

| 模板字段 | 本块实例差异、来源及缺口 |
| --- | --- |
| 关联、分类、调用模型、实现 ID | FUNC-RES 五组均为标量选择契约；ABS 复用首块 `scalar.abs.signed.checked/v1` 候选；SUBSTR 仅作 unknown 选择代表，实际规范实现 ID 待函数族取证；AMBIG 的两签名仅为待新增解析夹具，非生产函数或支持声明。 |
| 名称/别名/可见性、重载、cast | PG 使用 pg_catalog 内建，AMBIG 使用受控候选集；无新增别名。MySQL 走自身 ABS/SUBSTR，不应用 PG OID/候选优选。候选顺序依据 PG18 §10.3；当前双方言解析、工厂与完整参数定型未验证。 |
| NULL、返回类型、wire、错误 | 见验收表；ABS strict 属性继承首块来源。成功一列 v，PG 表/属性号 0、typmod -1，NULL 长度 -1，逻辑可空性另记。拒绝项无成功值、NULL 或结果载荷。PG 42883 为无匹配，42725 为歧义，参数不能定型/冲突沿第 3 章，不能把合法未实现统一伪装成参考端无匹配；P/完整消息及具体失败阶段待采集。 |
| collation、时区、会话、确定性 | 固定 UTF-8、PG 内建可见且无同名遮蔽；整数 ABS 不涉及 collation/时区，SUBSTR ASCII 输入排除 locale 差异但其完整排序规则未取证。ABS immutable 继承首块；SUBSTR 与夹具的 volatility/折叠批准不由本块推定，夹具仅解析不求值。MySQL 记录自身 sql_mode/charset/collation，运行隔离未验证。 |
| 下推、副作用、证据 | 本块不批准下推或新增折叠优化；成功 SELECT 无数据副作用，夹具解析无生产写入。源码与测试只读，PG/AsterSQL/MySQL 运行及性能均未验证。完整 wire 字段、错误字段继承首块，MySQL 精确 flags/charset/length/decimals 和诊断需补参考记录，不留作默认等价。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§10.3 Functions](https://www.postgresql.org/docs/18/typeconv-func.html) 给出上述签名选择顺序、variadic/default 规则及 substr unknown 例；[§9.3 数学函数](https://www.postgresql.org/docs/18/functions-math.html)支持 ABS 输入/返回同族；[PREPARE](https://www.postgresql.org/docs/18/sql-prepare.html)支持参数由上下文定型；[§54.2.3 扩展查询](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-EXT-QUERY)与[§54.7 消息格式](https://www.postgresql.org/docs/18/protocol-message-formats.html)支持分析、绑定、描述与执行分责；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)核对 42883/42725/42P18/42P08 分类。上述文档规则和受控候选推导是目标依据；未采集 18.6 参考端，不能记为补丁版本差分通过。

#### 等价共享与别名门槛

COALESCE 与 NULLIF 都只是**等价共享候选**，本块无批准项。批准单位是已解析参数类型向量、结果身份及适用输入域，不是函数名；COALESCE 还记录参数数目、顺序和共同类型，NULLIF 记录选中 `=` 的左右签名及其转换。即使复用算法，也分别保留方言名称、类型选择和 wire 映射。PG COALESCE 是特殊条件表达式，不按普通函数重载查找，不能将前块普通函数排序或 `pg_catalog.coalesce(...)` 当作等价入口，也不接受显式 VARIADIC 数组；NULLIF 的比较资格按运算符规则消费，不能硬套 COALESCE 的共同类型。

逐签名批准必须具备：双方言自身参考端和 AsterSQL 正式 SQL 入口的值/NULL、共同类型或比较提升、返回宽度/精度、求值顺序与错误时点、collation、wire、折叠/缓存及适用下推证据；记录完整版本、会话环境和命令。字段缺证保持“目标/待证”，不等价则保留方言分流。批准某个 int4/int4 域不授权 int8、numeric、text、unknown、其他参数数目或别名；IFNULL 的两个参数与 COALESCE 的相似算法不足以在 PG 公开 IFNULL。

以下实例引用“三层身份与契约模板”的全部字段，不适用和缺口明确如下。

| 模板字段 | 本块约束与状态 |
| --- | --- |
| 能力/用例、分类、调用模型、规范实现 ID | FUNC-EQUIV 五族，分类为第一类候选；标量结果条件表达式。候选语义 ID `conditional.first-non-null/v1`、`conditional.null-if-equal/v1` 为文档身份，非已注册符号。共享必须保留惰性子表达式边界；不能以预先计算全部参数的普通调用 ABI 代替。 |
| 名称、别名、方言可见性、重载、cast | 两入口目标开放自身 COALESCE/NULLIF 拼写，无新增别名；PG COALESCE 按共同类型、NULLIF 按 `=` 及其第一操作数提升，MySQL 保持自身转换/返回规则，不能套 PG OID。unknown、typed NULL 和参数分别消费第 3 章，不按运行值选型。MySQL 精确混型签名/元数据待参考端核对。 |
| NULL、求值顺序、返回身份 | COALESCE 左到右至首个非 NULL，全 NULL 返回分析固定类型的 NULL；非 strict，不能遇任意 NULL 就返回。NULLIF 相等返回 NULL，否则返回隐含比较后的第一参数；NULLIF(1,2.2) 的 PG 结果是 numeric，不能固定原始 int4。任一比较参数 NULL 时不构成 true；NULLIF(NULL,1)→NULL、NULLIF(1,NULL)→1，均 int4。具体输入见验收表。 |
| collation、时区、会话 | 数值例无时区/collation 运算；text 例显式固定 PG UTF-8/C 与 MySQL utf8mb4_bin，并采集 provider/版本及结果 collation。COALESCE 不比较文本仍须推导输出 collation，NULLIF 比较须消费所选 `=` 的 collation；不能以首个返回字符串决定规则。时间类型不在本块域内。名称/方言隔离沿前块。 |
| 确定性、折叠、副作用、下推 | 条件结构不一概标 immutable/strict，属性继承实际子表达式及比较操作符。验收 SELECT 无数据写入；以行变量避免把运行期短路测试退化成常量折叠，另采规划期拒绝。没有批准重排、重复求值、跨方言下推或新增折叠优化；volatile/副作用子表达式的完整开放仍须独立证据。 |
| wire、错误与证据 | PG 成功一列 v，表/属性号 0，示例 typmod -1；int4 为 23/4/-1、text 为 25/-1/-1、numeric 为 1700/-1/-1，NULL 长度 -1。格式/Describe 沿首块；逻辑可空性另记。错误记录 S/V/C/M 与适用 D/H/P、分析/规划/执行时点，完整消息/P 待采；MySQL code/flags/charset/length/decimals、错误/警告均待自身参考采集。无运行或性能证据。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.18 条件表达式](https://www.postgresql.org/docs/18/functions-conditional.html)给出 COALESCE 短路、共同结果类型及 NULLIF 的隐含比较/返回提升；[§10.5 共同类型](https://www.postgresql.org/docs/18/typeconv-union-case.html)用于 COALESCE 的 unknown 与已定型输入选择；[§23.2 collation](https://www.postgresql.org/docs/18/collation.html)规定显式冲突、输出派生及 C 排序。常量折叠与运行顺序边界另据 [§4.2.14 表达式求值规则](https://www.postgresql.org/docs/18/sql-expressions.html#SYNTAX-EXPRESS-EVAL)，错误分类沿本章[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)来源。表中推导均为待验收目标；未采集 18.6 参考端，不声称补丁版本差分通过。

#### 同名异义分流

同名拼写不是共享签名。CONCAT 的 NULL 策略、类型到文本的转换、返回身份须按连接方言分别选择；GREATEST/LEAST 还要分开参数下限、共同类型及比较规则。可复用已定型非 NULL 字符串缓冲拼接、或已获等价证明的比较/极值步骤；NULL 策略、coercion、返回类型和错误必须留在各自方言契约边界，不能全局修改现有 MySQL kernel 来满足 PG。

PG GREATEST/LEAST 是特殊表达式，不是普通 pg_catalog 函数重载，不接受显式 VARIADIC 数组；普通函数候选排序不直接适用。PG concat 是普通可变参数函数，不能与同样跳过 NULL 的 MySQL CONCAT_WS 当别名，后者另有分隔符契约。以下实例引用首块全部模板字段，不批准任何新别名或下推。

| 模板字段 | 本块方言目标、来源与当前缺口 |
| --- | --- |
| 关联、分类、调用模型、规范实现 ID | 第二类“同名异义”，关联 FUNC-SPLIT 四族。文档候选 `string.concat.skip-null/pg/v1` 与 `string.concat.propagate-null/mysql/v1`、`extremum.ignore-null/pg/v1` 与 `extremum.propagate-null/mysql/v1` 分离；最大/最小和参数类型向量另记。标量结果，PG 极值为特殊表达式；这些 ID 不是已注册 Rust API。 |
| 名称、别名、可见性、重载、隐式 cast | 两入口均目标可见自身 concat/greatest/least；PG concat 可用 pg_catalog 限定名，极值用非限定特殊语法。PG concat 使用各参数类型的文本输出，不要求参数共同类型；极值依 §10.5 在分析期确定共同类型并只用第 3 章 I 资格。MySQL 保持 ETString/聚合类型及自身转换。unknown/typed NULL/参数不按运行值选型；完整可变数组边界另验，无新增别名。当前正式接线未验证。 |
| NULL、求值条件、返回身份和值域 | PG concat 忽略 NULL，全 NULL 的普通调用返回非 NULL 空串/text；MySQL 任一 NULL 返回 NULL/自身字符串类型。PG 极值忽略 NULL，全 NULL 返回共同类型的 SQL NULL；MySQL 任一 NULL 传播。PG 比较需要所有非 NULL 候选，不承诺沿用 MySQL 遇 NULL 停止；错误顺序不从局部 Datum 哨兵推导 SQL 行为。PG 极值返回共同类型，不根据胜出值重新定型。依据官方规则及局部源码；全链未验证。 |
| collation、时区、会话 | 拼接不排序，但结果字符属性须推导；极值按输入派生 collation 比较。文本域固定 PG UTF-8/C、MySQL utf8mb4_bin，并记录 provider/版本。数值/文本代表无时间运算，时区对本组不适用；不推广到其他类型的输出函数，PG concat 的任意类型输出可能依赖会话。MySQL packet/sql_mode/字符设置另采，不能套给 PG。 |
| 确定性、折叠、副作用 | REL_18_0 的 concat 为非 strict、stable，不能按纯缓冲算法标为 immutable；极值结构继承子表达式/比较的属性，无独立 pg_proc volatility。验收 SELECT 无写入；不批准重排、重复求值或新增折叠规则。MySQL 保留原优化属性，完整 volatile/错误阶段需后续取证。 |
| 返回修饰符、wire、错误 | 本组 PG text 为 25/-1/-1、int4 为 23/4/-1、numeric 为 1700/-1/-1（OID/typlen/typmod），列 v、表/属性号 0，NULL 长度 -1；逻辑可空性独立记录。MySQL 数值目标 BIGINT/DECIMAL，字符串保持自身列定义；code/flags/charset/length/decimals 与精确 precision/scale 待参考采集，不由 PG 类型倒推。错误采 S/V/C/M 和适用 D/H/P、实际阶段；成功无错误字段。目标码见验收，完整消息/P 未采集。 |
| 下推、验收、证据状态 | 只允许未来逐签名/输入域证明等价后共享非 NULL 算法；TiPB 名称不能授权 PG NULL/coercion 下推。四族逐行核对两入口，当前只有静态源码、断言意图和官方目标，无参考端/AsterSQL 实测，无性能结论。 |

官方依据（2026-10-23 查阅，版本 PostgreSQL **18**）：[§9.4 字符串函数](https://www.postgresql.org/docs/18/functions-string.html)定义 concat 文本输出和忽略 NULL；[§9.18.4 条件表达式](https://www.postgresql.org/docs/18/functions-conditional.html#FUNCTIONS-GREATEST-LEAST)定义极值的特殊调用、NULL 与共同类型；[§10.5 类型选择](https://www.postgresql.org/docs/18/typeconv-union-case.html)支撑混型/全 unknown 推导；[§23.2 collation](https://www.postgresql.org/docs/18/collation.html)支撑 C 排序与派生。固定 **REL_18_0** 的 [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)核对 concat 的 any 可变参数、text、非 strict/stable；[gram.y](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/parser/gram.y)的 GREATEST/LEAST expr_list 产生 MinMaxExpr，支持单参数和空列表语法拒绝的目标推导。错误分类沿首块附录 A；这些均非 18.6 补丁版本实测。

#### 时间截断与提取

本块引用首块统一模板；PG 核心专属的是 date_trunc/date_part 及 PG 的提取语义，EXTRACT 语法在 MySQL 入口也可见但必须分流。EXTRACT 的 AST/字段语法交第 2 章，timestamp/timestamptz/interval 身份及 codec 交第 3 章，本块只规定已定型表达式的语义交接，不批准把 PG EXTRACT 别名到现有整数工厂。

| 模板字段 | 方言目标、依据与当前缺口 |
| --- | --- |
| 关联、分类、模型、实现 ID | 待新增能力族 FUNC-TEMP、标量。文档规范候选 `scalar.pg.temporal.truncate/{timestamp,timestamptz,interval}/v1` 与 `scalar.pg.temporal.field/{timestamp,timestamptz,interval}/{numeric,float8}/v1` 分开保留返回策略；不是现有 API。date_part/EXTRACT 可候选共享字段算法，精度和身份仍独立，未批准共享实现。 |
| 名称/别名、可见性 | PG 普通调用 date_trunc/date_part 支持 pg_catalog 限定及第 2 章名称环境；EXTRACT(field FROM source) 使用语法交接。MySQL 不开放 date_trunc/date_part，保留自身 EXTRACT；隔离为 AsterSQL 设计门禁，MySQL 参考端名称拒绝细节未采。无新增跨方言别名。 |
| 重载、cast | date_trunc(text,timestamp)→timestamp、(text,timestamptz)→timestamptz、(text,timestamptz,text zone)→timestamptz、(text,interval)→interval；三参显式区域属于 timestamptz 签名，timestamp/interval 本身没有带 zone 的截断重载。date_part(text,T)→float8、EXTRACT 对应已定型 T→numeric，T 为本块三种类型。参数先按前块及第 3 章 I 资格选择，不靠值猜类型；显式 timestamp→timestamptz 转换的时区依赖必须保留。date/time 自动转换遵循官方规则但不是本块代表验收域，不扩展到 timetz。 |
| 单位、值域与精度 | timestamp 截断支持 microseconds、milliseconds、second、minute、hour、day、week、month、quarter、year、decade、century、millennium，低位清零，日/月必要时置 1，week 从 ISO 周一开始；interval 按分量截断，不支持 week。SECOND 提取包含小数，MICROSECONDS 包含完整秒乘一百万；interval 的 month 是年内余月、day 是日分量，不能将月固定折秒后再提字段。date_part 浮点可能损精度，EXTRACT 以 numeric 保留微秒；截断不是四舍五入。非有限值沿第 3 章保留：合法截断保留 infinity；提取单调字段返回对应 infinity，其他合法字段为 NULL，不能误作 strict 导致的 NULL；具体扩展域需后续聚焦补证。 |
| NULL 与求值 | 本块各签名 strict，任一已定型实参 NULL→其固定返回类型的 SQL NULL；三参 zone=NULL 也为 NULL，不调用非 NULL 内核。NULL 不绕过签名/语法分析；不保证其他实参的分析或折叠错误被跳过。依据 pg_proc.dat 与 pg_proc.h 默认属性，当前全链未验证。 |
| collation、时区、会话 | 不做用户文本排序，collation 对时间计算不适用，字段名按 PG 单位解码而非 locale 比较；固定 UTF-8、DateStyle=ISO,YMD、IntervalStyle=postgres，记录 TimeZone、tzdata 与输入 p。timestamp/interval 按原字段，timestamptz 默认用会话区域；三参截断用指定区域求瞬时，结果显示仍用会话区域。提取 timestamptz 的 HOUR/timezone 等依会话区域，epoch 是绝对瞬时秒数；timestamp epoch 是忽略时区/DST 的名义秒数，二者不能互换。 |
| 确定性、折叠、副作用 | REL_18_0：两参 timestamptz 截断、timestamptz 提取为 stable；显式 zone 的三参截断、timestamp/interval 截断及提取为 immutable。无数据副作用；折叠与计划缓存须保持这些属性及区域配置边界，不从 kernel 无状态推断全部 immutable。当前缓存/折叠隔离未验证。 |
| 返回、wire 与修饰符 | 按签名固定结果，未额外 cast 的本块表达式目标 OID/typlen/typmod：timestamp 1114/8/-1、timestamptz 1184/8/-1、interval 1186/16/-1、date_part float8 701/8/-1、EXTRACT numeric 1700/-1/-1。不能由输入 p 或截断单位重造输出 typmod，numeric 的显示 scale 不等于 typmod。列名 AS v、来源表/属性号 0、格式与 NULL 长度 -1 沿首块；MySQL EXTRACT 走自身整数/复合单位和列定义，精确 flags/length/decimals 待参考采集。所有本块函数 wire 仍缺运行证据。 |
| 下推、错误与证据 | 默认不批准 PG 下推；现有 ExtractDatetime/Duration 的 TiPB 分支不证明 PG 精度/时区等价。未知字段目标 22023，已识别但类型不支持如 interval 的 week 截断、timestamp 的 timezone 提取目标 0A000；无匹配签名 42883，时区错误与输入/范围错误分开采集。错误 S/V/C/M、适用 D/H/P、阶段按首块，精确消息/位置和本地映射未验证。下方五族均为目标/静态意图，无功能或性能通过证据。 |

官方来源（2026-10-23 查阅，PostgreSQL **18**）：[§9.9.1 EXTRACT/date_part](https://www.postgresql.org/docs/18/functions-datetime.html#FUNCTIONS-DATETIME-EXTRACT)分别明确 numeric 与 double precision 返回、字段及精度区别；[§9.9.2 date_trunc](https://www.postgresql.org/docs/18/functions-datetime.html#FUNCTIONS-DATETIME-TRUNC)定义类型、单位和区域行为。精确重载、返回和 volatility 核对官方固定 **REL_18_0** [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)，strict/immutable 缺省核对 [pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h)；单位适用性、错误及字段算法核对 [timestamp.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/timestamp.c) 的 timestamp_trunc、timestamptz_trunc_internal、interval_trunc、timestamp_part_common、timestamptz_part_common。OID/长度核对 [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)，typmod/格式沿第 3 章。以下 DST 数值为按区域规则推导的验收目标，尚无参考端采样；18.6 仍仅是来源指定目标，18 手册及 18.0 静态源码不能记作该补丁版本运行证据。

#### 格式化函数契约

引用首块统一模板，to_char 是 PG 核心专属标量，DATE_FORMAT 是 MySQL 专属；共享日历/数字组件只能作为候选，不能开放跨方言别名。下列实现 ID 仅为文档身份，不预设新 API。

| 模板字段 | 方言目标、依据及当前缺口 |
| --- | --- |
| 关联、分类、调用模型、实现 ID | 待新增 FUNC-FMT 五族；标量候选 `scalar.pg.format.temporal/v1` 与 `scalar.pg.format.numeric/v1`，按已选源类型保留各自语义；MySQL `scalar.mysql.date_format/v1` 独立。当前仅源码/断言意图与官方规则，无运行证据。 |
| 名称/别名与可见性 | PG to_char/pg_catalog.to_char 遵循第 2 章名称环境，MySQL 隐藏；DATE_FORMAT 在 MySQL 保留，在 PG 隐藏。该隔离为 AsterSQL 设计门禁，不推断参考端自定义函数不存在；无新增别名。 |
| 重载与隐式 cast | 精确签名 `(T,text)→text`，T 为 timestamp、timestamptz、interval、int4、int8、numeric、float4、float8。无直接 text 源重载；date/int2 等候选必须经前块 resolver 和第 3 章 I 资格，不能把 MySQL 字符串日期宽松转换搬过来。unknown/NULL/参数 OID 0 不由值猜重载；代表用例均显式定型。 |
| 模板与修饰符 | 时间与数值模板各自解析，不能互换；代表 YYYY/MM/DD、HH24/MI/SS/US、ISO IYYY/IW、数值 9/0/小数/符号、字面量、FM/TH/TM 见下方验收。FM 对时间只修饰下一项；数值 FM 是数值格式模式，不能套时间规则。FX 的输入解析约束不属于本块，SP 未实现不得承诺。 |
| NULL 与求值 | 所列重载均 strict，已定型任一参数 NULL→NULL/text；NULL 不绕过签名分析或保证其他实参不折叠。时间源空模板及非有限时间值返回 NULL 的额外边界来自 formatting.c，不能把此行为推广为所有数值空模板策略。 |
| collation、locale/时区 | UTF-8；TM 月/日名称依 lc_time，D/G/S 依 lc_numeric，L 依 lc_monetary；固定标点模板 . 和 , 不随 locale 换字符。记录输入/输出 collation，第 3 章派生规则保留，不把排序 collation 等同 locale 设置。timestamptz 先按会话 TimeZone 映射墙钟和偏移，timestamp/interval 保留自身字段；记录 tzdata、OS locale 库与可用名称。 |
| 确定性、折叠、副作用 | 官方目录将上述全部 to_char 重载标 stable，不能因某个纯数字模板看似固定便整体标 immutable。无数据副作用；准备语句跨执行的 locale/TimeZone 改变不得被旧结果缓存掩盖。折叠/缓存隔离未验证。 |
| 返回、wire、修饰符 | 所有成功重载均 text，PG OID/typlen/typmod=25/-1/-1，不是 varchar(n)，模板长度/输入精度不生成 typmod。列名 AS v、表/属性号 0，text/binary 均为编码后的文本字节；空格/尾零不得 trim。NULL 长度 -1、零行描述和 statement/portal 格式沿首块，可空性单记；MySQL 字符串使用自身 code/flags/charset/length/decimals，精确字段待采。 |
| 下推、错误与状态 | 无下推批准；日期百分号/FORMAT locale 内核不能证明 PG 等价。非法数值模板 V 与小数点组合→42601（formatting.c::NUMDesc_prepare），无匹配签名→42883；普通未知文本按字面量处理，不能一概报非法模板。错误采 S/V/C/M、适用 D/H/P 与分析/折叠/执行阶段；确切消息/位置、本地映射及 MySQL 自身警告/错误待采。 |
| 验收/证据状态 | 下方五族分别记录两入口输入、值/NULL/类型/错误及可见性；全部待新增、未运行，缺参考端 18.6 实测，成功项 SQLSTATE 不适用。不得据文档结构或名称表升级支持。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.8 格式化函数及表 9.26–9.31](https://www.postgresql.org/docs/18/functions-formatting.html)给出模板和示例；[§19.11.2 locale 设置](https://www.postgresql.org/docs/18/runtime-config-client.html#RUNTIME-CONFIG-CLIENT-FORMAT)定义配置前提。精确签名/stable 核对固定 **REL_18_0** [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)，strict 缺省核对 [pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h)。[formatting.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/formatting.c)的 timestamp_to_char、timestamptz_to_char、interval_to_char、NUMDesc_prepare 支持时间边界和错误；结果身份/协议沿已取证的第 3 章 text 契约。18.6 仍仅为来源指定目标，18 手册和 18.0 静态源码不构成该补丁版本差分记录。

#### 数组函数契约

以 array_length、cardinality、array_append 为 PG 核心专属的**普通标量函数**目标，不承诺全量数组库；三层模板沿首块，规范实现候选分别为 `scalar.array.dimension-length/v1`、`scalar.array.cardinality/v1`、`scalar.array.append-element/v1`，均为文档身份，尚无获准共享项。值表示、元素族/typmod、矩形 shape、下界、空数组与两层 NULL 继承第 3 章基础数组契约。维数不改变数组类型 OID；不将多维值建模为“数组的数组”。

| 模板字段 | 本块方言目标、来源与缺口 |
| --- | --- |
| 关联/分类/调用模型、名称/别名/可见性 | FUNC-ARR 五组关联上述候选；PG 可见非限定名与 pg_catalog 限定名，MySQL 隐藏三个 PG 名称；无新增别名。普通标量一输入行对应一个结果，unnest 不适用本模型。依据官方签名和前块名称规则；本地正式接线未验证。 |
| 重载、隐式 cast、元素定型 | `array_length(anyarray,int4)→int4`、`cardinality(anyarray)→int4`；`array_append(anycompatiblearray,anycompatible)→anycompatiblearray`。前两者消费已知数组元素身份；后者在分析时将数组元素与追加元素按第 3 章 I 资格定共同元素类型，再转换整数组及元素，返回该元素对应数组。多态伪类型不是 wire OID；unknown/typed NULL 不按运行值猜型。本块只实例化 int4[]，并用 int4[]+int8 检查提升，不补全多态系统；不兼容的定型类型对不可因 NULL 绕过。 |
| NULL | array_length/cardinality strict：数组 SQL NULL 得 SQL NULL；array_length 的维号 NULL 也返回 NULL。array_append 非 strict：NULL 数组作为空数组，NULL 元素作为一个真实空位追加；空数组、数组 NULL、元素 NULL 不互换。依据固定 tag 目录/实现；当前未实测。 |
| collation、时区、会话 | 对已定型数组的计数/形状和本块 int4/int8 追加不适用 collation/时区（不比较文本、不处理时刻）；元素输入、输出及 cast 仍沿第 3 章元素族设置。名称解析沿连接上下文，不能由纯算法推导全链无会话依赖；跨连接隔离未验证。 |
| 确定性、折叠、副作用 | 三者目标 immutable，无 SQL 数据副作用，常量可规划期折叠；不能把内部原地优化理解为修改输入 SQL 值。本地折叠、缓存及错误阶段未验证。 |
| 返回类型、值域与 wire | 两计数结果 int4，23/4/-1；append 本组 int4[] 为 1007/-1/-1，提升 int8[] 为 1016/-1/-1，分别依赖元素 23/20 与数组 OID 配对。AS v、来源表/属性号 0；NULL 或零行不改身份。shape/lower 属值而非 OID/typmod；binary 数组头的元素 OID、维长/下界、元素长度/NULL 及两格式输出沿第 3 章。RowDescription 无可空性位；逻辑可空性另记。当前返回身份/codec 贯通未知，MySQL 不暴露这些 PG 数组 OID。 |
| 下推/执行位置 | 默认无下推或跨方言别名批准，须逐签名证明形状、NULL、cast、错误及元数据等价；现有 JSON append、目录 codec 均不足以批准。性能不适用本轮文档检查，无运行结论。 |
| 错误、验收与证据 | 五组成功无错误；无效维号返回 NULL 是成功。多维 append 目标 22000，下界追加索引溢出 22003；不匹配名称/签名沿 resolver，精确混型诊断另采，不统一套某个错误码。错误采首块 S/V/C/M 与适用 D/H/P、原 SQL 位置和分析/折叠/执行阶段，无伪成功行。FUNC-ARR 均待新增、未运行；官方目标与源码/断言意图不能代替双方言当前记录。 |

官方来源（2026-10-23 查阅）：PostgreSQL **18** [§9.19 数组函数](https://www.postgresql.org/docs/18/functions-array.html)定义代表函数，[§8.15 数组](https://www.postgresql.org/docs/18/arrays.html)支持形状/下界与 NULL，[§36.2.5 多态](https://www.postgresql.org/docs/18/extend-type-system.html#EXTEND-TYPES-POLYMORPHIC)支持分析期共同元素类型。固定 **REL_18_0** 的 [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)与[pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h)核对签名、strict/immutable；[arrayfuncs.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/arrayfuncs.c)::array_length/array_cardinality 核对维号与计数，[array_userfuncs.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/array_userfuncs.c)::fetch_array_arg_replace_nulls/array_append 核对 NULL、下界和错误分支。数值例为上述规则推导的待验收目标；18.6 仍为来源指定基线，未在该补丁版本实测。

#### 范围函数契约

沿首块统一模板，本块仅实例化 PG 核心专属普通标量 lower/upper/isempty 与 int4range 构造，以 numrange 作连续域对照；实现 ID 候选 `scalar.pg.range.bound/v1`、`scalar.pg.range.empty/v1`、`scalar.pg.range.construct/v1` 仅为文档身份。range 是单个 SQL 值，不是行集合或两个独立标量；multirange 不在本块。

| 模板字段 | 本块目标、依据及缺口 |
| --- | --- |
| 关联/分类/名称/别名/可见性 | 待新增 FUNC-RNG 四组；PG 非限定名及 pg_catalog 限定名沿第 2 章名称环境可见。MySQL 隐藏 range 重载、isempty/int4range/numrange，保留 lower/upper 自身字符串调用；不得按名称全局屏蔽或把字符串大小写函数当边界函数。无共享别名批准，当前正式接线未验证。 |
| 重载/隐式 cast/值域 | `lower(anyrange)→anyelement`、`upper(anyrange)→anyelement`、`isempty(anyrange)→boolean`；多态返回由分析时已知 range 的 subtype 决定，非 wire 伪类型。本组构造 `(int4,int4[,text])→int4range`、`(numeric,numeric[,text])→numrange` 是两个/三个参数的独立签名；两参默认 [)。unknown/参数 OID 0 及隐式转换沿 resolver 和第 3 章 I 资格，不根据值/NULL 猜 subtype。其他 range 族不在代表验收域内。 |
| 表示与边界 | 必须分别保存 range/subtype 身份、整体 SQL NULL、empty、端点是否无界及开闭标志；端点值不是可存 SQL NULL 的成员槽。有限端点大小顺序必须合法。离散 int4range 规范化后再观察端点；连续 numrange 保留开闭方式，不能套整数步长。第 3 章没有该表示/codec 承诺，需先单独补齐窄类型契约才可实施函数。 |
| NULL/求值 | lower/upper/isempty strict，range SQL NULL→相应已定型结果的 SQL NULL。empty 的 lower/upper 为 NULL、isempty 为 true；无界仅对应端点提取为 NULL，isempty 为 false。构造器非 strict，任一端点 NULL 表示该侧无界，两端 NULL 是全域；三参 flags=NULL 则错误，不是返回 NULL。NULL 不绕过签名分析或保证其他实参不折叠。 |
| collation/时区/会话 | 本组 int4/numeric 不做文本排序或时刻换算，collation/TimeZone 对值运算不适用；固定 UTF-8，数值输入输出沿第 3 章。名称与类型选择仍依连接上下文；不推广到文本自定义 range 或 timestamptz 的会话显示。 |
| 确定性/折叠/副作用 | 所列函数/构造签名按固定目录默认属性为 immutable，无 SQL 数据副作用；合法常量可以折叠，错误可在分析/折叠/执行出现，不强制延迟到 Execute。本地缓存、折叠与隔离未验证。 |
| 返回/wire/修饰符 | 本组 lower/upper：int4 为 23/4/-1，numeric 为 1700/-1/-1；isempty 为 16/1/-1；构造为 int4range 3904/-1/-1 或 numrange 3906/-1/-1。依签名固定 OID/typlen/typmod，空/NULL/零行不改身份；AS v、表/属性号 0、NULL 长度 -1、Describe 格式沿首块，逻辑可空性另记。range 文本应输出规范边界或 empty；binary 为标志字节及存在的端点长度/子类型二进制载荷，不发送内部 varlena/OID 结构，也不透传文本字节冒充 binary。参数/结果 codec、身份连续性是未具备的依赖。 |
| 下推/错误/证据 | 默认无下推批准；构造下界大于上界、NULL flags 目标 22000，非法 flags 目标 42601，int4 离散规范化端点溢出目标 22003；名称/签名无匹配沿前块 42883。采 S/V/C/M、适用 D/H/P 和实际阶段，无伪成功行；精确本地映射、消息/位置及 MySQL 错误待采。工程缺口不能冒充参考 PG 的语义错误；所有场景未运行。 |

官方来源（2026-10-23 查阅，PostgreSQL **18**）：[§9.20 range 函数](https://www.postgresql.org/docs/18/functions-range.html)核对提取/empty，[§8.17.3–7 range 类型](https://www.postgresql.org/docs/18/rangetypes.html)核对构造、无界和离散规范化。固定 **REL_18_0** [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)及[pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h)核对精确签名、strict 和 immutable；[pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)核对 OID/长度；[rangetypes.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/rangetypes.c)的 range_constructor2/3、range_lower/upper、range_parse_flags、range_serialize、int4range_canonical、range_send 核对 NULL、错误、规范化与 binary 布局。数值例是规则推导目标；18.6 仍为来源指定版本，未采该补丁版本参考端证据。

#### 必需系统函数契约

沿首块统一模板，本块定义五个普通标量候选，规范实现候选 ID 为 `scalar.pg.system.database/v1`、`schema/v1`、`version/v1`、`backend-pid/v1`、`typeof/v1`（后四项沿同一 `scalar.pg.system.` 前缀）；不是已注册 ID。优先级依据上表客户端探测静态证据，先数据库/schema，再版本、连接身份与类型内省；后面三项的具体客户端版本、调用频率和需求紧迫度未采，不能仅因候选名存在便升级支持。

| 模板字段 | 本块目标、事实提供者及缺证边界 |
| --- | --- |
| 关联/分类/调用模型/名称/可见性 | 关联下方 FUNC-SYS 五组；五个 PG 签名和 pg_catalog 限定名沿第 2 章可见，current_schema 无括号形式由语法层承接。version 同名按方言分流；其余不向 MySQL 开 PG 别名，MySQL DATABASE()/VERSION()/CONNECTION_ID() 保留原契约。不批准跨方言共享别名。当前通用标量链未验证。 |
| 重载/隐式 cast | current_database()→name、current_schema()→name、version()→text、pg_backend_pid()→int4；pg_typeof("any")→regtype，"any" 是接受实参类型的伪类型而非 wire 类型，不把输入都转 text，也不按物化值猜类型。显式 typed NULL 和 Parse 参数类型优先，unknown/参数 OID 0 沿前块解析规则单列；本组不承诺未取证的任意域/用户类型解析。 |
| 值/NULL/事实责任 | 数据库身份由工作流 5/7 的所属连接提供；schema 由工作流 5 的有效显式路径与工作流 6 的对象存在/USAGE 事实共同决定，无有效项返回 SQL NULL。版本取真实构建/兼容性声明提供者，工作流 7 保持启动参数一致，不伪造参考 PG 构建串。PID 取工作流 7 的 backend 身份分配与取消注册表，不能强转 MySQL u64 或共享进程 PID。pg_typeof 取第 3 章分析后的参数类型身份，类型名/OID 映射归工作流 6；typed NULL 仍返回类型的非 NULL regtype，不走 strict NULL 传播。其余四项零参数，参数 strict 快捷不适用；正常连接数据库/版本/PID 非 NULL。 |
| 权限/会话/collation/时区 | 固定 UTF-8；值不做时区换算或文本比较，collation 对本组值运算不适用，name/text 自身类型与字符属性仍保留。schema 必须忽略不存在或无 USAGE 的路径项，不能把路径字符串当可见对象事实；隐式 catalog 不自动成为 current_schema。角色/权限变化的有效路径刷新由 5/6 提供，不新增权限查询函数。PG 数据库在连接内固定，改库须重连，不借 MySQL USE 实现 PG 身份切换。当前仅 public/catalog 的路径模型不证明完整权限过滤。 |
| 确定性/折叠/副作用/下推 | 固定 PG 目录五项均为 stable，无 SQL 数据写入；pg_typeof 非 strict，其余无实参。结果依连接、类型或服务器环境，不能按 immutable 跨会话缓存；准备语句不能冻结 current_schema 的后续 SET。内省不保证消除实参正常求值/错误；不得以 pg_typeof 重写吞掉表达式副作用。下推默认不批准，事实须在所属 SQL 会话读取，无性能结论。 |
| 返回类型/wire/修饰符 | current_database/schema 为 name（OID 19，标准 NAMEDATALEN=64 的 typlen 64）；version 为 text（25/-1）；PID 为 int4（23/4）；pg_typeof 结果列为 regtype（2206/4），其值可为 int4 的 OID 23，不能把结果列 OID 写成 23。本组 typmod 目标 -1；name/regtype 身份与 text/oid 不可混同，第 3 章未承诺部分先补窄类型/codec 依赖。AS v、表达式表/属性号 0、NULL 长度 -1、描述格式与零行规则沿首块；name 二进制不发内部定长填充，regtype 二进制为 OID 载荷。MySQL 精确 code/flags/charset/length/decimals 待参考采集，不能从 PG OID 推导。 |
| 错误/阶段/证据 | 合法调用无错误，错误字段不适用；如 current_database(1)、pg_typeof() 无匹配签名，PG 目标 42883，分析阶段拒绝且无成功行。无参数类型可确定时另采 resolver 错误，不猜 SQLSTATE；基础设施/工厂/codec 缺失记工程缺口，当前目录 0A000 不冒充 PG 语义。诊断沿首块记录 S/V/C/M、适用 D/H/P 和原 SQL 字符位置，精确消息/P/本地映射待采。全部运行未验证。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.27 系统信息](https://www.postgresql.org/docs/18/functions-info.html)定义五项返回语义；[§19.11.1 search_path](https://www.postgresql.org/docs/18/runtime-config-client.html#GUC-SEARCH-PATH)定义有效路径与权限过滤。固定 **REL_18_0** 的 [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)核对签名、stable 及 pg_typeof 非 strict，[misc.c::pg_typeof](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/misc.c)核对从表达式类型取得 OID，[pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)核对 name/int4/text/regtype 身份及长度；[§54.7 消息格式](https://www.postgresql.org/docs/18/protocol-message-formats.html)核对描述与 BackendKeyData。标准 name 长度及 binary 布局另核对同 tag 的 [pg_config_manual.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/pg_config_manual.h)、[name.c::namesend](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/name.c) 和 [regproc.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/regproc.c)；本地 codec 未验证。18.6 仅为来源指定版本，没有该补丁版本差分结果。

#### MySQL专属函数隔离

沿首块统一模板，四个标量契约关联下方 FUNC-MY 四组，规范实现候选 ID 分别为 `scalar.mysql.ifnull/v1`、`scalar.mysql.date-format/v1`、`scalar.mysql.json-extract/v1`、`scalar.tidb.build-info/v1`；仅为文档身份，不声称已注册。MySQL/TiDB 原语义以本仓库 Go 工厂与求值为保留基线；PG 相似能力不构成改写依据。

| 模板字段 | 四类目标、来源与缺证边界 |
| --- | --- |
| 名称/别名/可见性 | MySQL 入口保留 IFNULL、DATE_FORMAT、JSON_EXTRACT、TIDB_VERSION；PG 入口不公开这些内建名称，包括 pg_catalog 限定名。按第 2 章方言与名称环境先过滤候选，再选择工厂，NULL 或常量折叠不能绕过门禁。此为 AsterSQL 设计边界，不声称参考 PG 无法创建同名用户函数。无跨方言别名；开放例外只能逐签名满足“等价共享与别名门槛”（任务 4），不能直接挂 COALESCE、to_char、json_extract_path 或 version。 |
| 重载/隐式 cast | IFNULL 固定两参，继承 Go 控制类型聚合、按结果求值类型转换及宽度/flags；不套 PG COALESCE 类型规则。DATE_FORMAT 两参按 MySQL datetime/string 转换。JSON_EXTRACT 至少两参，首参 JSON、其余字符串路径，遵循 Go 类型资格检查与转换。TIDB_VERSION 零参，不适用参数 cast。unknown/typed NULL 的 PG 调用也先拒绝名称，不能调用 MySQL cast 选出签名。 |
| NULL/求值/错误 | IFNULL 首参非 NULL 不求值备用分支，首参 NULL 才取后值，全 NULL 为推导结果类型的 SQL NULL；已到达参数的错误正常传播。DATE_FORMAT 任一参数 NULL→SQL NULL，非法/零日期保持 Go sql_mode 下警告或错误，空模板是非 NULL 空串。JSON_EXTRACT 的 SQL NULL 文档或路径→SQL NULL、缺失→SQL NULL，存在 JSON null 是非 SQL NULL 的 JSON 值；非法 JSON/路径仍报自身错误。TIDB_VERSION 无参数 NULL 场景（零参），正常结果非 NULL，额外 NULL 参数是参数数目错误。 |
| collation/时区/会话 | IFNULL 保留 MySQL 字符集、collation 派生与数值/时间修饰符；DATE_FORMAT 保留 MySQL 日期转换、会话时区及实际 locale 规则，不能换用 PG 模板或 lc_*。JSON 路径按自身语法，不借 PG jsonpath 或文本 collation 改义。TIDB_VERSION 读本服务构建信息，不依参数 collation/时区，返回字符串仍携自身字符属性。固定并记录 sql_mode、charset/collation、time_zone 和构建基线；精确派生与非默认环境运行证据未采。 |
| 确定性/折叠/副作用 | 前三项仅在输入转换及所属会话上下文相同时才讨论复用/折叠，IFNULL 保留短路与错误时点；不从局部纯函数推断 SQL immutable。TIDB_VERSION 无数据写入，值属于服务构建上下文，不能用 PG version 值替换或跨不同构建缓存。精确 Go/Rust 折叠及缓存策略未完整取证，保持原策略，后续须补优化证据。 |
| 返回类型/值域/修饰符/wire | IFNULL 按两参聚合结果，本文 CHAR 用例为 MySQL 字符串；DATE_FORMAT 为 MySQL 字符串及 Go 模板长度推导；JSON_EXTRACT 为 MySQL JSON（TypeJSON/code 245），不是 PG json/jsonb OID；TIDB_VERSION 为 MySQL 字符串、Go flen 为 GetTiDBInfo 字节长度。其余 code/flags/charset/length/decimals 精确值待参考采集，不能从 Rust String 猜协议类型。NULL、零行与重绑值不能改列类型。PG 名称拒绝无成功结果类型/RowDescription，不分配伪造 text/JSON OID。 |
| 下推/错误字段/证据 | 不新增下推批准，不改变既有 MySQL 下推策略；已存 TiPB 签名不批准 PG 下推。MySQL 参数数目、转换、路径错误按自身 code/SQLSTATE/消息/警告和实际阶段采集，精确字段尚未采；PG 隐藏名称目标分析阶段 42883，无成功行，记录 S/V/C/M 与适用 D/H/P，精确位置/消息未采。缺工厂是工程缺口，不能当作门禁成功。成功错误字段不适用；当前全链有效运行用例为 0。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.18 条件表达式](https://www.postgresql.org/docs/18/functions-conditional.html)仅把 IFNULL 作为其他数据库的相似能力，并说明 COALESCE 不是普通函数；[§9.16 JSON](https://www.postgresql.org/docs/18/functions-json.html)区分 SQL NULL/JSON null，列出的 json_extract_path 与 jsonpath 不授权 MySQL JSON_EXTRACT 别名。[§10.3 函数解析](https://www.postgresql.org/docs/18/typeconv-func.html)支持先筛可见候选，[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)定义 42883 为 undefined_function。这里的 MySQL 语义来自本地 Go，PG 隔离来自设计契约；18 大版本手册不能代替 18.6 实测。

#### 运算符调用契约

继承“三层身份与契约模板”，调用模型独立记录为运算符/比较谓词：规范算法身份、方言运算符签名和 wire 结果身份仍分三层。PG 候选身份至少含 namespace、符号、一元前缀或二元中缀及左右输入类型；一元缺左操作数是结构信息，不是传入 SQL NULL。结果类型来自已选签名，不参与反向选重载；内部 div/intdiv/nulleq/concat 名称不能自动成为 PG SQL 别名。`IS [NOT] DISTINCT FROM` 是特殊比较谓词，需按底层 `=` 的比较资格绑定并专门处理 NULL，不伪装成用户可查找的同名普通函数或运算符对象。

PG 运算符候选按第 2 章可见环境筛选：限定 `OPERATOR(pg_catalog./)` 只取指定 schema，未限定符号取可见同名/同元数候选；同输入签名按路径去重，不同输入签名平等考虑。先精确匹配；二元一侧 unknown 时在此步骤先假设其与已知侧同型，一元 unknown 无此捷径。其后按第 3 章 I 转换资格、精确位置数、类别优选、unknown 的 string 类偏好和已知同型约束逐步筛选，不套普通函数的 variadic/default 规则或 UNION 共同类型规则。无匹配目标 42883、歧义 42725；domain/完整多态等未开放载体保留能力门禁，不能借本块构建完整类型系统。Parse/Prepare 定型后，Bind 换值、NULL 或格式不得重选运算符。

| 首块模板字段 | 本块实例填写及证据缺口 |
| --- | --- |
| 关联、分类、模型、规范实现 ID | 关联下方 OP-CALL 四族。文档候选 `operator.integral.divide.checked/v1`、`operator.numeric.divide/v1`、`operator.text.concat.strict/v1`、`predicate.int.null-safe-equal/v1` 为待新增实现身份；二元比较的有限 int4 值域可作共享候选，不批准跨方言别名。MySQL `/` 与 PG 整数 `/` 为同符号异义；三个代表行为均未有两入口运行证据。 |
| 名称/可见性、元数、输入、cast | `/`、文本 `\|\|` 为二元，`-` 为一元前缀；签名和限定名见正文/验收表，均限定内建无同名用户/扩展遮蔽。算术只取 int4/int4、numeric/numeric 两签名；文本取 text/text，不将 anynonarray 混合拼接泛化为本块支持；NULL 安全比较限定 int4 对。无批准别名，unknown 与 typed NULL 按上述规则参加分析，不由空值绕过非法类型。 |
| NULL、求值条件 | PG 两个除法重载与 text/text 拼接 strict，合法 typed NULL 返回同型 NULL；NULL::int4 / 0::int4 不进入非空除零算法。NULL 安全谓词非 strict，结果永不为 NULL；普通 `=` 为对照。strict 不承诺子表达式求值顺序或避免规划期常量错误，不以 `(1/0)` 包在其他 NULL 表达式下作必然免错断言。 |
| collation、时区、会话 | 固定 UTF-8、内建可见、PG search_path 和 MySQL sql_mode 逐次记录；数字运算及 int4 比较不涉及 collation/时区。text/text 只拼接原字符，不按排序规则改变字符，结果 collation 派生消费第 3 章，字符集/collation 冲突须另取证，不能宣称任意 collation 已兼容。MySQL `\|\|` 分别固定 PIPES_AS_CONCAT 关/开，不能让该配置影响 PG。 |
| 确定性、折叠、副作用、下推 | 已定型内建 int4 除法与 textcat 按官方 REL_18_0 默认属性为 immutable/strict；其余实例仅限定固定输入身份的纯值目标，不批准新增优化属性。SELECT 无数据副作用；可折叠常量的值错误可发生于规划，参数错误亦须记录真实阶段。不新增下推许可，类型、NULL、错误及会话差异未证明等价前不下推；无性能结论。 |
| 返回类型、值域、wire | 下表列值/NULL/类型；int4 域与溢出不扩宽沿第 3 章，一元负最小值及最小值除 -1 目标 22003。PG 结果 OID/typlen：int4 23/4、numeric 1700/-1、text 25/-1、bool 16/1，表达式 typmod -1，列名 v、表/属性号 0；text/binary 按第 3/7 章，不由返回行推型。MySQL 沿自身 DECIMAL/BIGINT/字符串及布尔整数表达式元数据，精确 code/flags/charset/length/decimals 待参考采集；不输出 PG OID。 |
| 错误、验收、状态 | 除零 22012、域溢出 22003，无匹配/歧义见上；成功 SQLSTATE/错误字段不适用，拒绝无成功载荷。沿首块记录 S/V/C/M 与适用 D/H/P，精确消息/P 和分析/折叠/执行阶段未采集；MySQL 警告/错误按自身参考，不能把合法 PG 未实现伪装成 PG 无匹配。只完成静态取证和契约，双方言运行有效用例数为 0。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§10.2 运算符解析](https://www.postgresql.org/docs/18/typeconv-oper.html)与[pg_operator](https://www.postgresql.org/docs/18/catalog-pg-operator.html)支持候选/元数及输入、结果、实现函数身份分离；[§9.3 数学运算](https://www.postgresql.org/docs/18/functions-math.html)定义整数向零截断及非整数除法，[§9.4 字符串运算](https://www.postgresql.org/docs/18/functions-string.html)定义 text 拼接，[§9.2 比较谓词](https://www.postgresql.org/docs/18/functions-comparison.html)定义 NULL 安全比较。固定 **REL_18_0** [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat) 的 int4div/textcat 与 [pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h) 的 strict/immutable 默认值支持对应属性；[int.c::int4div](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int.c)支持除零/最小值除 -1，[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持错误分类。18 大版本手册与 18.0 静态源码不等于 18.6 差分证据，本轮没有参考端运行结果。

#### 普通聚合调用契约

继承“三层身份与契约模板”，普通 aggregate 独立于 scalar/window/ordered-set/SRF：分析绑定可见聚合签名与固定结果身份，执行器为每个分组创建状态、逐行 transition、最后 finalize；server 只编码最终值。规范实现候选 `aggregate.count.nonnull/v1`、`aggregate.sum.typed/v1`、`aggregate.avg.sum-count/v1` 是文档身份，不是新增 API 或共享批准。输入、过渡状态和返回类型必须分别记录；下面的状态是所需逻辑信息，不要求复制 PG 内部布局。

| PG 代表输入签名 | 逻辑过渡状态与最终结果目标 |
| --- | --- |
| count(*)、count(单个已定型标量) | 非负计数，初值 0；前者计输入行，后者仅计非 NULL。返回 int8，即使无行仍为非 NULL 0。星号是调用形式，不是 NULL 参数；多表达式 COUNT 不借 MySQL 多参规则开放。 |
| sum(int2/int4)、sum(int8/numeric) | 保存是否见非 NULL 及精确和；前组返回 int8，后组返回 numeric。状态须足够保存目标结果及溢出信息，不按输入整数宽度截断；无有效输入返回同型 SQL NULL。 |
| avg(int2/int4/int8/numeric) | 保存非 NULL 计数及足够精度的和，最终一次相除返回 numeric；不能平均各局部平均数，也不能继承 MySQL div_precision_increment 作为 PG 精度规则。无有效输入返回 numeric NULL，不进行除零。 |
| sum(float4/float8)、avg(float4/float8) | 保存浮点累计所需信息和有效计数；SUM 分别返回 float4/float8，AVG 均返回 float8。不能因共享 f64 容器改写 SUM(float4) 身份；相加顺序、舍入及非有限值策略须另取运行证据，不承诺任意重排等价。 |

FILTER 是每个聚合自己的输入门禁，仅 true 的行进入该聚合的参数/去重/排序和状态更新，false/NULL 均排除；不等于整条查询 WHERE，不能删掉其他聚合或已有分组。DISTINCT 按已定型输入的相等/排序资格去重，不能把编码字节相等直接当 PG 等价；本组 count/sum/avg 忽略 NULL。聚合内 ORDER BY 负责 transition 输入次序，独立于最终结果行 ORDER BY；DISTINCT 与排序并用时，排序表达式只能引用 DISTINCT 参数列表。普通聚合可无排序，WITHIN GROUP 及 OVER 另块处理。空全局聚合仍产一行，空分组输入没有组/没有结果行；已形成的组即使 FILTER 全排除也须保留其 0/NULL 结果。

| 首块模板字段 | 本块填写与缺口 |
| --- | --- |
| 关联/分类/名称/别名/可见性 | 关联 AGG-CALL 五组与上述规范候选；PG count/sum/avg 及 pg_catalog 限定名按第 2 章可见，MySQL COUNT/SUM/AVG 保留自身聚合调用。相同名称不批准跨方言别名、状态或最终化共享。两入口正式 PG 签名接线未验证。 |
| 重载/隐式 cast | 只覆盖表列数值和 count 的单标量/星号；unknown、typed NULL 与参数 OID 0 沿前块 resolver 和第 3 章 I 资格，Bind 换值不选新签名。显式 text 不擅自转 numeric 以通过 sum；interval/money 等其他重载不在本代表验收域。 |
| NULL/求值条件 | 按上述行级 NULL/无行/组规则；最终 aggregate 不能套标量 strict(NULL) 直接返回 NULL，count 必须返回 0。FILTER 排除行不执行该行 transition；常量子表达式仍可能提前折叠报错，不承诺 SQL 从左到右求值或规避所有规划期错误。 |
| collation/时区/会话 | 本组数值和整数分组不涉及文本 collation 或时区算术，输入转换、名称解析仍消费连接环境；count 的通用文本 DISTINCT/分组须另证明 collation 等价，本轮不泛化。记录编码、MySQL sql_mode/div_precision_increment，PG 不继承后者。 |
| 确定性/折叠/副作用 | 固定输入多重集和合法次序下的纯聚合目标，无数据写入；依赖行集合，不能因参数为常量当成单次标量折叠。浮点重排可能改变结果，不新增 immutable/parallel-safe、跨连接缓存或消除排序许可；具体优化属性未取证。 |
| 返回/值域/修饰符/wire | 按上表固定身份：PG int8 20/8/-1、numeric 1700/-1/-1、float4 700/4/-1、float8 701/8/-1（OID/typlen/typmod，沿第 3 章）。输入 numeric(p,s) 不强行继承结果 typmod；NULL/无行不改描述，过渡状态不发给客户端。MySQL COUNT 为自身 BIGINT、精确数 SUM/AVG 为 DECIMAL、近似数为 DOUBLE；精确 flags/length/decimals/charset 待参考采集。 |
| 局部/最终/共享/下推 | 默认不新增许可；分别证明状态初值、跨组隔离、合并与空局部、精度/溢出/错误时点及最终类型。AVG 合并 sum/count 后一次最终化；跨分区重复值不能只相加局部 DISTINCT 结果，ORDER BY/FILTER 不能丢失。PG 手册支持 partial mode 不证明 AsterSQL 的 Split、typed 路径或 TiPB 下推已等价。 |
| 错误字段/阶段/证据 | 合法项成功无错误；无匹配 sum(text) 目标 42883。非法 DISTINCT/排序组合须拒绝，无成功行，精确 SQLSTATE/消息/位置待参考采集；计数及有界整数结果溢出不能 wrapping 或变 NULL，PG 数值越界沿第 3 章 22003。记录实际分析/折叠/transition/final 阶段及 S/V/C/M、适用 D/H/P；MySQL 保持原 code/SQLSTATE/警告，具体映射未采。有效运行用例 0。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.21 聚合函数](https://www.postgresql.org/docs/18/functions-aggregate.html)定义三种代表的输入、返回、NULL 及 partial-mode 能力；[§4.2.7 聚合表达式](https://www.postgresql.org/docs/18/sql-expressions.html#SYNTAX-AGGREGATES)定义星号、DISTINCT、FILTER 与聚合内排序；[§36.12 状态/最终化与 partial aggregation](https://www.postgresql.org/docs/18/xaggr.html)区分输入/状态/结果类型及 combine 责任。协议、类型和错误字段沿本章模板及第 3 章。以上不是 18.6 的运行证据，精确补丁版本与参考端仍未核实。

#### 窗口函数调用契约

沿首块全部模板字段，本块仅选 row_number/rank、lag/lead、last_value；规范实现候选 `window.position/v1`、`window.partition-offset/v1`、`window.frame-last/v1` 是文档身份，未批准共享或别名。窗口保留每个输入行，不能改成普通聚合的一组一行，亦不能由标量工厂逐行独立求值。

| 模板字段 | 方言目标、来源与当前缺口 |
| --- | --- |
| 名称/可见性/重载/cast | PG 内建 row_number()/rank()→int8；lag/lead(value anyelement [,offset int4])→anyelement，三参数 (value anycompatible,offset int4,default anycompatible)→共同 anycompatible（手册合并展示，目录区分实际重载）；last_value(anyelement)→同输入类型。PG 名称及 pg_catalog 限定名按前块 resolver；MySQL 同名按自身规则独立选择，不能套 PG 多态类型。仅消费第 3 章 I 转换资格；本块值域固定 int4，混型多态全域不扩写，实际两入口接线未验证。 |
| 分区/排序/peer | PARTITION BY 独立重置状态，省略则所有输入为一个分区；同序 peer 仅按窗口 ORDER BY 全部键的相等语义判定，外层 ORDER BY 只排序输出。PG 默认 ASC NULLS LAST、DESC NULLS FIRST；显式 NULLS 与文本 collation 归第 3 章比较身份。row_number 对 peer 内分配不保证固定，rank 同 peer 同值且跳号；需确定位置时给窗口追加唯一键，但不可因此改变 rank 的 peer 夹具。 |
| 默认/显式帧 | PG 默认 RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW，终点包含当前最后 peer；无 ORDER BY 时全分区均为 peer，默认覆盖全分区。ROWS 按物理行，RANGE 按排序值，GROUPS 按 peer 组；RANGE offset 要且仅要一个排序键，offset 类型随键（时间键用 interval）。ROWS/GROUPS offset 非 NULL 非负整数，GROUPS 要求排序。边界截在分区内；合法反向距离可形成空帧，不等于空分区。 |
| 求值/NULL/offset | row_number/rank 按分区位置、不受帧限制；lag/lead 按分区位置取前/后行、不受帧限制，offset/default 相对当前行求值，省略为 1/typed NULL。offset=0 取当前值，NULL offset 返回同型 NULL，负 offset 反转方向（PG 源码）；越界才取 default，目标行值为 NULL 不触发 default。last_value 取帧末行值，空帧或末值 NULL→同型 NULL；均 RESPECT NULLS，PG 18 未实现 IGNORE NULLS。不能套 strict 标量的“任一 NULL 即不调用”捷径。 |
| 确定性/上下文/副作用 | 对固定分区、全序输入与表达式上下文给出目标结果；无全序时不冻结 peer 内位置，无 ORDER BY 不承诺位置次序。不将有 OVER 的调用折叠成标量常量或跨分区缓存；offset/default 表达式保留自身 volatility/错误规则，不承诺副作用调用次数。本块整数夹具不依时区，文本排序/时间 RANGE 另依 collation/时区身份；优化器属性与上下文隔离未验证。 |
| 返回/wire | PG 排名结果 int8，OID/typlen/typmod=20/8/-1、非 NULL；本块 int4 lag/lead/last_value 为 23/4/-1、逻辑可空，default 类型先参与分析，不能由实际 NULL/零行换型。列名显式别名、表达式表/属性号 0，Q/statement/portal、text/binary 与 NULL 长度 -1 沿首块。MySQL 自身整数/值类型及 code/flags/charset/length/decimals 待参考采集；UInt 容器不能决定 PG wire。 |
| 下推/拒绝/错误 | 不批准下推或跨方言共享。GROUPS、EXCLUDE、带类型 RANGE offset 等未实现形式须在能力门禁显式拒绝，不能丢弃/改 ROWS 或全分区；这表示实现缺口，不表示 PG 拒绝合法 SQL。非法负/NULL 帧 offset、非法起止类别组合、缺 OVER、IGNORE NULLS 均须拒绝；精确 SQLSTATE/消息/位置及阶段待参考采集，不把内部 Ok(None) 当 SQL 错误证据。按首块采 S/V/C/M、适用 D/H/P 和分析/规划/执行阶段，MySQL 保留自身错误/警告；成功无错误。 |
| 关联/验收/证据 | 关联 WIN-CALL 五组及上述三实现候选、静态基线；源码/独立断言意图已读，真实 SQL 工厂→物理执行→server 桥接、两协议与参考端运行证据均缺，有效运行用例 0。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.22 窗口函数](https://www.postgresql.org/docs/18/functions-window.html)定义签名与 NULL/帧责任；[§4.2.8 窗口调用](https://www.postgresql.org/docs/18/sql-expressions.html#SYNTAX-WINDOW-FUNCTIONS)定义帧模式、边界与限制；[§7.2.5 窗口处理](https://www.postgresql.org/docs/18/queries-table-expressions.html#QUERIES-WINDOW)定义处理阶段与排序责任。固定 **REL_18_0** [windowfuncs.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/windowfuncs.c) 的 window_row_number/window_rank/leadlag_common/window_last_value 支持位置、NULL/负 offset 和空帧边界，[pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)核对窗口种类及多态签名；NULL 默认排序沿第 3 章。18.6 仍仅为来源指定目标，18 手册/18.0 源码不构成该补丁版本差分证据。

#### 有序集合聚合契约

本块引用“三层身份与契约模板”，只以单比例 `percentile_cont`、`percentile_disc` 定义 ordered-set 模型。目标能力候选 `FUNC-ORDERED-CONT/pg`、`FUNC-ORDERED-DISC/pg`，规范实现候选分别为 `ordered.percentile.interpolate/v1` 与 `ordered.percentile.select/v1`；这些是文档身份，不是现有 API、函数对象 OID 或支持声明。

| 模板字段 | 方言目标、依据与当前缺口 |
| --- | --- |
| 关联、分类、名称/别名及可见性 | PG 专属 ordered-set，名称为 percentile_cont/percentile_disc 及 pg_catalog 限定形式，依第 2 章环境选名；不开放 APPROX_PERCENTILE、普通聚合或窗口的跨模型别名。MySQL 入口不得经共享注册意外获得 PG 契约；其原有函数保留原规则，两入口可见性均未实测。 |
| 重载、隐式 cast 与返回类型 | 单比例 direct 参数为 double precision；cont 的排序输入 double precision→double precision 或 interval→interval；disc 的排序输入为可排序 anyelement，结果保留解析时确定的具体输入类型。仅消费第 3 章 I 资格，不由最终值、NULL 或 wire 容器反推签名；代表验收固定 cont float8 与 disc int4。interval 与其他可排序类型须各自补类型/codec 证据后开放，不以 MySQL Duration 等价代替；当前重载接线未验证。 |
| 调用载体与求值条件 | `percentile_cont(p) WITHIN GROUP (ORDER BY x ASC)`：p 是每组一次的 direct argument，x 是每输入行一次的聚合输入；p 引用本层变量须满足 GROUP BY 约束，不能把随行变化的比例偷偷取首行。ORDER BY 必需且参与签名、排序方向/比较语义；不能用普通聚合的可选排序或输入偶然有序代替。FILTER 只让谓词为 true 的行进入该聚合，false/NULL 不进入，不能改成全查询过滤。DISTINCT 与 OVER 不作为该调用的合法替代语法，非法形态须明确拒绝；当前 AST/分析/计划载体缺证。 |
| NULL、空集合、比例与结果 | 两者排除排序输入 NULL，保留非 NULL 重复值的频数；全 NULL 或空的全局聚合输出一行同型 SQL NULL，有 GROUP BY 且无组时为零行。NULL::float8 比例返回同型 NULL，合法范围为闭区间 [0,1]，越界须报错，不截断、不钳制、不回退默认值。非法比例与空集合组合的精确报错阶段仍须参考采集，不能因空输入擅自跳过比例验证。 |
| 排序、collation、时区及算法 | 有效样本排序后，disc 选第一个累计比例达到 p 的值（p=0 取首值）；cont 必要时对邻值插值，不能都实现成选样本。ASC/DESC 是调用契约；NULLS FIRST/LAST 不令被排除的 NULL 参与比例分母。代表数值不依赖 collation/时区；disc 文本等未来重载必须沿第 3 章比较/collation 资格，不能沿 MySQL 排序器默认规则猜测，interval 插值另核实。 |
| 确定性、折叠、副作用与状态 | 已定类型/比例/比较语义的代表输入应有可重复结果，无用户数据写副作用；这是整组运算，不能按标量逐行折叠。每组状态只接收聚合输入和排序条件，finalize 消费 direct 参数并选值/插值，结束后释放状态；不得把 p 当逐行 transition 输入。PG 文档说明 final 可能改变排序状态，不能未经证明共享或继续追加；缓存/重复 final 的具体策略当前缺证。 |
| wire 元数据与格式 | 沿第 3 章，由最终签名决定 PG float8 OID 701/typlen 8/typmod -1，disc int4 为 23/4/-1；interval 目标 1186/16/-1，codec 仍须单独证据。列别名统一 v，表达式来源表/属性号 0；NULL 长度 -1，零行也保留描述。核对 Q 与 Parse/Describe/Bind/Execute 的文本/二进制格式，不能暴露排序状态类型。MySQL 若拒绝调用则无成功结果列，错误沿其自身协议；当前两入口未采集。 |
| 下推与执行位置 | PG 两个代表聚合 Partial Mode 均为 No；本契约不批准 Split 成局部分位数后合并，也不批准跨方言下推。可复用现有分组/取消/资源生命周期入口，但须独立证明排序状态、NULL、结果身份和错误等价；不要求复制 PG 内部实现，排序内存、溢写与优化性能当前未测。 |
| 错误字段、阶段及证据 | 有效项成功无错误；非法比例目标为错误且无成功值，精确 SQLSTATE/主消息/位置及分析或 final 阶段待参考采集；NaN/Infinity、非法签名、缺 WITHIN GROUP、非分组 direct 变量、DISTINCT/OVER 分别记录参考错误，不能伪造码或让 panic 成协议行为。按第 1 章 S/V/C/M 与适用 D/H/P 记录，本块有效运行用例 0。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.21 表 9.64](https://www.postgresql.org/docs/18/functions-aggregate.html#FUNCTIONS-ORDEREDSET-TABLE)给出签名、NULL/比例及 partial-mode 规则；[§4.2.7](https://www.postgresql.org/docs/18/sql-expressions.html#SYNTAX-AGGREGATES)定义 WITHIN GROUP、direct 参数与 FILTER；[§36.12.3](https://www.postgresql.org/docs/18/xaggr.html#XAGGR-ORDERED-SET-AGGREGATES)说明 transition/final 的不同参数、排序状态与不能作为窗口调用。以上只支持大版本 18 目标，未建立精确 18.6 参考实例或差分结论；下列值是据规则推导的目标，不是数据库实测。

#### 集合返回函数契约

引用“三层身份与契约模板”，以 `generate_series` 整数序列和单数组 `unnest` 为首批候选，文档能力 ID 为 `FUNC-SRF-SERIES/pg`、`FUNC-SRF-UNNEST/pg`，规范实现候选为 `srf.series.integer/v1`、`srf.array.elements/v1`；不是现有 API 或已批准共享。每次调用返回零至多行，各行列结构在分析时确定，不能把集合塞入一个标量值或用一行 NULL 代替零行。

| 模板字段 | 方言目标、依据及证据缺口 |
| --- | --- |
| 分类、名称/别名、可见性 | PG 专属 SRF，generate_series/unnest 及 pg_catalog 限定形式按第 2 章解析；MySQL 入口不得由共享注册意外获得此 PG 调用模型，已有 MySQL 行为保留。无跨方言别名批准，两入口隔离未实测。 |
| 重载、隐式 cast、列数/返回身份 | generate_series(int4,int4[,int4])→setof int4、int8 同型重载→setof int8，省略步长为 1；单参数 unnest(anyarray)→setof anyelement，解析后固定具体元素类型。代表验收固定 int4/int4[]、一列 v；仅消费第 3 章 I 资格，不由首行或 NULL 推断类型。多数组 unnest 仅 FROM 合法，每数组贡献一列且短数组补 NULL；这是独立扩展边界，不把它当标量多参重载。 |
| NULL、值域与错误 | 序列参数任一 typed NULL 产生零行；步长符号与区间方向相反产生零行，端点在步进能到达时包含，零步长须错误。unnest 的 NULL 数组及空数组产生零行，数组中的 NULL 元素产生一行 SQL NULL，按存储次序展开，重复元素保留。整数边界终止须避免加法回绕/无限循环；边界用例与 NULL/零步长混合的错误优先级待参考采集，不伪造结果。 |
| 调用位置与关联语义 | FROM 中是行源；函数实参可引用左侧输入，LATERAL 关键字可省略。每个关联输入重新建立调用状态，CROSS JOIN 空集合消除外行，LEFT JOIN LATERAL … ON true 则保留外行并补 NULL。目标也包含 SELECT 投影中的单 SRF 展开：每外行耗尽后再处理下一外行，空集合不输出该外行；不是普通 project_row。多个投影 SRF 同步取第 n 项，短者补 NULL，行数取最长者；CASE/COALESCE 内的 SRF 须在分析阶段拒绝，不静默截成首值。多个/嵌套 SRF 的实施需独立证据后开放。 |
| 生命周期、确定性、折叠与副作用 | 整数序列及已定型数组展开对固定输入可重复、无用户数据写副作用；不把所有时间重载据此定为 immutable。每调用持有游标/结束标记与固定输出描述，耗尽、重扫、取消、错误、提前 LIMIT 和关闭均须释放或重置；协议暂停后恢复不得丢行/重复或串到下一外行。可优化常量集合但不得按一个标量折叠，资源实现可复用既有执行器，不另建 PG 执行系统；以上均未运行。 |
| collation、时区及独立重载 | 代表 int4 序列/数组展开不进行文本排序或时间计算，collation/时区不适用；unnest 保留元素及其类型身份，不借 MySQL 字符串重排。numeric 序列、timestamp+interval→timestamp、timestamptz+interval[+timezone]→timestamptz 仅列独立重载边界，后者涉及显式 timezone 或会话 TimeZone 与 DST；不得由整数候选推断时间重载已支持。 |
| wire 元数据与格式 | 返回每个元素而非数组/游标状态；int4 为 OID 23/typlen 4/typmod -1，int8 为 20/8/-1，unnest int4[] 结果为 23/4/-1。AS v 固定列名，表达式来源表/属性号 0；零行仍可 Describe，NULL 载荷长度 -1，无 wire 可空性位。Q 与 Parse/Describe/Bind/Execute 分别核对文本/二进制、参数身份、实际行数及暂停恢复，MySQL 拒绝时无成功结果描述；当前两入口未采集。 |
| 下推、错误字段与证据 | 默认不批准跨方言下推或用标量 TiPB 签名代替 SRF；须证明基数、顺序、NULL、溢出终止、关联重扫与取消等价。零步长目标错误、无成功值；非法签名/位置分别采集 S/V/C/M 与适用 D/H/P 和分析/执行阶段，精确 SQLSTATE、消息、位置未取参考证据，不把目录 0A000 当 PG 通用语义。正确性/兼容性仅为目标，无性能结论，有效运行用例 0。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.26](https://www.postgresql.org/docs/18/functions-srf.html)给出序列签名和方向/步长/NULL 规则；[§9.19](https://www.postgresql.org/docs/18/functions-array.html)定义 unnest 元素展开与多数组形态；[§7.2.1.4–5](https://www.postgresql.org/docs/18/queries-table-expressions.html#QUERIES-TABLEFUNCTIONS)定义 FROM、关联函数与外连接；[§36.5.9](https://www.postgresql.org/docs/18/xfunc-sql.html#XFUNC-SQL-FUNCTIONS-RETURNING-SET)定义投影中的集合展开、同步补 NULL 及条件构造拒绝。目标值由这些规则推导，没有精确 18.6 参考实例或差分结果；wire 字段沿第 3 章与本章首块，不用大版本手册提升运行证据等级。

#### 章节验收与后续交接

下表是来源覆盖索引；块名均指本章同名四级标题，场景 ID 指“验收”中的具体输入/目标表，不能用标题命中代替逐字段审阅。

| 来源要求 | 正文落点 | 验收落点 |
| --- | --- | --- |
| 步骤 1：规范实现、方言签名、协议返回三层；复用规范内核 | 三层身份与契约模板；注册工厂与方言可见性；重载与隐式转换选择 | FUNC-ID 四族、FUNC-REG 四族、FUNC-RES 五组，区分缺名/缺工厂/无匹配/歧义/同值异型 |
| 步骤 2 第一类：等价共享及别名门槛 | 等价共享与别名门槛，COALESCE/NULLIF 特殊表达式不误套普通函数 | FUNC-EQUIV 五组，包含共同类型、未达错误与 collation；共享/别名批准仍为 0 |
| 步骤 2 第二类：同名异义 | 同名异义分流，concat/greatest/least 的 NULL、类型与调用形式 | FUNC-SPLIT 四族，双方言独立输入及各自预期 |
| 步骤 2 第三类：PG 核心专属 | 时间截断与提取、格式化函数契约、数组函数契约、范围函数契约、必需系统函数契约、集合返回函数契约 | FUNC-TEMP/FMT/ARR 各五组、FUNC-RNG 四组、FUNC-SYS 五组、SRF 五组；date_trunc/date_part/to_char/数组/range/generate_series/系统代表均落地 |
| 步骤 2 第四类：MySQL/TiDB 专属 | MySQL专属函数隔离，IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION | FUNC-MY 四组及 FUNC-REG-MYSQL-ONLY；自身成功/NULL/参数错误与 PG 名称拒绝分别取证 |
| 步骤 3：全部模板字段及逐签名下推批准 | 首块统一模板及各块差异表；本块下方门禁 | 下方双入口维度矩阵；各场景未定义 wire/诊断精确值明确保留缺口 |
| 步骤 4：六种调用模型、系统事实边界 | 标量链、运算符调用契约、普通聚合调用契约、窗口函数调用契约、有序集合聚合契约、集合返回函数契约；系统事实归 6 | FUNC-ID、OP-CALL 四组、AGG-CALL 五组、WIN-CALL 五组、ORDERED 四组、SRF 五组及各模型生命周期验收 |
| 交付要求：模块/目标/范围外/验收/后续细分，先注册与重载再分族 | 本章五个全局小节及本块后续交接 | 17 块×五节结构核对、引用存在性、来源映射及人工差异审阅 |

所有实例继续引用首块模板；本块是覆盖索引，不另造一个遗漏字段的函数实例。字段逐项审阅包括：关联/实现候选/场景/分类/调用模型；名称/别名/方言可见性；重载/隐式 cast；NULL/求值条件；collation/时区/会话；确定性/折叠/副作用；返回身份/值域/修饰符；wire/格式；下推/执行位置；错误字段/失败阶段；证据等级。整数 collation/时区不适用须保留纯整数原因，拒绝项成功值/NULL/结果列不适用须保留拒绝原因；MySQL 精确列字段、部分 PG 诊断/阶段、range 基础身份、非 C locale 等未知项仍是显式缺口，不能因文档完成填成默认值或运行通过。

六种模型分别保留标量单值求值、运算符元数/签名与谓词、普通聚合每组 transition/final、窗口每输入行的分区/帧、有序集合 direct 参数与排序状态、SRF 零至多行及关联重扫。可共享算法或生命周期入口，不以统一标量注册抹平基数、状态、排序与错误时点。

下推批准须以“方言＋已解析签名＋适用输入域＋目标存储/版本”为单位，先满足现有能力/黑名单/编码门禁，再有两入口及自身参考端的值、NULL、返回身份、cast、collation/时区、错误/警告与时点等价证据；补折叠/缓存/会话隔离和本地执行对照。普通聚合另证 DISTINCT/FILTER/排序、状态合并与最终类型，窗口另证分区/帧，SRF 另证基数/次序/重扫/取消；有序集合仍不批准局部分位数合并。缺任一必要证据保持未批准，不因 TiPB 或白名单存在放行；本轮新增下推批准为 0，无性能结论。

本块官方复核（2026-10-23，PostgreSQL **18**）：[§10.3 函数解析](https://www.postgresql.org/docs/18/typeconv-func.html)核对名称环境/输入签名选择；[§4.2 表达式](https://www.postgresql.org/docs/18/sql-expressions.html)核对运算符、聚合、窗口及 WITHIN GROUP 不同调用形式；[§36.5.9 集合返回](https://www.postgresql.org/docs/18/xfunc-sql.html#XFUNC-SQL-FUNCTIONS-RETURNING-SET)核对集合行展开边界；[§54.7 协议消息](https://www.postgresql.org/docs/18/protocol-message-formats.html)核对类型、格式与 NULL 描述。这些只支持上述有限规则；具体函数语义沿各块官方引用，不从本块四页推广整个函数库。18.6 仍为来源指定目标，未核实该补丁版本参考实例或获得运行差分。

### 范围外

#### 三层身份与契约模板

本块不实现生产函数、注册接口、Rust 类型、类型系统、协议 codec、下推或测试运行器，不运行 Cargo、Go 测试、数据库或差分环境；不盘点函数库。四类函数的完整实例、别名开放与重载排序、运算符及 aggregate/window/ordered-set aggregate/SRF 的独立调用模型留给本章后续文档块，不用标量注册器代替其他调用模型。系统事实归工作流 6，会话/事务与消息生命周期归工作流 5/7；不重写第 1–3 章、不补建扩展加载生态。

#### 注册工厂与方言可见性

本块不实现名称解析、方言注册 API、扩展管理或工厂补齐，不补齐 ABS 工厂，也不把共享字符串表改造成完整 pg_proc。PG/MySQL 函数族的全部参数语义、重载优先级、多态/variadic/default 参数、目录可见对象和协议状态机分别交后续块及工作流 6/7；不启动数据库、不运行 Cargo/Go 或新增测试平台。

#### 重载与隐式转换选择

本块不实现候选排序器、转换矩阵、参数推断、UDF、默认/多态函数或完整类型系统，不补工厂、不改共享 NULL 推导，也不生成生产代码/测试。只交接第 3 章既有资格的消费方式；运算符的专门候选规则、各函数族值语义及协议状态机归后续块和工作流 7。受控歧义夹具只要求未来解析器测试注入候选，不要求新增 SQL CREATE FUNCTION 生态。

#### 等价共享与别名门槛

只定义两种条件表达式的逐签名批准规则和五组用例，不实现工厂、NULLIF 重写、类型/collation 解析器、惰性 ABI、协议或下推，不补齐当前缺工厂，不盘点其他函数。域/数组/时间/用户自定义 `=` 的完整支持及 volatile 子表达式生态不在本块代表域内，不能因未覆盖而扩大本轮实现。不会运行 Cargo、数据库或差分平台。

#### 同名异义分流

本块不实现函数、不修补缺工厂、不补建 PG 特殊表达式/类型系统/协议或测试平台，不穷举可变数组、时态、JSON、locale/输出函数；不变更 MySQL 原 NULL、混型、参数和 packet 行为。这些域需分别补证，不从当前代表用例批准全函数共享。

#### 时间截断与提取

只定义三种时间源的截断/字段提取代表契约，不实现工厂、字段解码、时间身份、tzdata 服务、会话生命周期、AST、codec、下推或差分平台；不扩写 date_bin、格式化、日历运算及完整 date/time/timetz 函数库。第 2/3 章及注册/重载前块既有缺口分别交接，不因本块需要而补建；不运行 Cargo、数据库或构建/代码 lint。

#### 格式化函数契约

不实现 to_date/to_timestamp/to_number 解析器，不扩展完整模板函数库、locale 安装器或时区系统；不修复已读 MySQL 工厂/测试缺口、不建立新的求值器或测试平台。不批准 DATE_FORMAT/FORMAT 别名、PG 下推或全域性能结论；范围外缺口留给各自聚焦实现。

#### 数组函数契约

不实现数组表示、函数工厂、多态解析、cast/codec 或测试平台，不扩大为完整数组库。unnest 归集合返回模型，数组运算符归运算符任务；下标/切片、数组聚合、索引、自定义/复合/domain 元素不在本块。第 3 章拥有类型/形状与 codec，工作流 2/5/6/7/8 分别拥有语法、会话、目录、协议与客户端证据；不改 Go/MySQL JSON_ARRAY_APPEND 的原契约，不把缺口改写成 PG 语义拒绝。

#### 范围函数契约

本块不实现 range 表示、构造/求值、cast、codec 或测试设施，不修改第 3 章，不将基础类型缺口扩大为子系统建设。multirange、自定义 range、其他 range 族的完整验收、范围运算符/索引/聚合及扩展生态均在本块范围外；两入口已有 lower/upper 字符串契约另行保留。本轮不运行 Cargo/Go、数据库或差分平台。

#### 必需系统函数契约

不实现代码、工厂、OID 分配、任意目录行或权限系统；不补建完整系统函数库、角色生态、统计视图、版本伪装或取消协议。注册/求值只消费事实接口，目录对象/OID/权限事实归工作流 6，会话路径/角色归 5，连接启动/身份/取消归 7，类型身份归第 3 章。现有探测解析与 SQL 适配不是通用实现方案，禁止用 SQL 文本替换、固定 public/库名/进程号或参考 PG 版本串让探测通过；不把目录当前占位 NULL 当权限事实。

#### MySQL专属函数隔离

不盘点全部 MySQL/TiDB 管理函数，不修复 DATE_FORMAT/TIDB_VERSION 工厂或测试缺口，不改 Go 语义，不补建 JSON、时区、权限或管理子系统。不新增 COALESCE/to_char/PG JSON 别名，不把其他函数或扩展生态纳入隔离实现；本块不修改代码、协议或下推。

#### 运算符调用契约

不实现 PG operator resolver、表达式工厂、cast、codec、目录或测试设施，不改 Go/MySQL 算法及 sql_mode。只取整数/非整数除法、文本拼接和 int4 NULL 安全比较；数组/range 运算符登记为后续独立族，不能借 `||` 同符号或 lower/upper 已注册推定支持。其他数值/文本混合重载、完整算术库、行/复合比较、用户自定义运算符及扩展生态不在本块。语法优先级和谓词节点归第 2 章，算术值域/规范类型归第 3 章，系统目录/协议生命周期归工作流 6/7；不运行 Cargo、数据库或差分平台。

#### 普通聚合调用契约

不实现 FILTER 载体、排序器、PG numeric 或新的聚合执行器，不修复两条现有聚合路径的精度/溢出差异，不扩展完整聚合库、分布式状态序列化或测试平台；窗口 frame/逆转移、WITHIN GROUP 的 direct arguments、有序集合及 SRF 留给独立调用模型。现有 MySQL/Go 行为继续保留，不用 PG 规则重写；共享、局部/最终和下推缺证分别交后续聚焦工作，不阻塞本块文档交付。

#### 窗口函数调用契约

不实现窗口解析器、PG 物理计划桥接、排序器、GROUPS/EXCLUDE、动态 offset/default、完整多态类型或 codec，不修复当前 MySQL 描述符/执行器差异。不扩展全部排名/窗口聚合、逆转移、磁盘溢写、分布式执行及测试设施；有序集合和 SRF 交后块，目录/协议生命周期分别交工作流 6/7。只定义能力拒绝边界，不以缺基础设施阻塞文档交付。

#### 有序集合聚合契约

hypothetical-set（rank、dense_rank、percent_rank、cume_dist 的 WITHIN GROUP 形式）单列范围外；同名窗口实现不证明其 hypothetical 行注入、direct/排序参数配对或状态可用。不实现 percentile 数组比例重载、mode、完整可排序类型库、PG 排序器/内存溢写、分布式 partial 状态、AST/协议代码或测试平台；不修复现有 APPROX_PERCENTILE 的 MySQL 行为。发现的类型/codec 缺口交对应边界，不递归补建子系统。

#### 集合返回函数契约

不实现 SRF AST/分析器、工厂/迭代器、LATERAL 计划、投影展开、数组类型/codec、协议 portal 或测试设施；不改目录特例、不扩大为完整函数库。时间/numeric 序列、多数组/复合记录输出、WITH ORDINALITY、多个及嵌套投影 SRF 留作独立扩展证据边界，不能在未实现时静默采用标量行为。类型/名称归第 2/3 章，目录事实和会话/协议生命周期归工作流 6、5/7；本块仅定义接口责任与代表目标。

#### 章节验收与后续交接

不实现函数、resolver、类型/codec、执行器、目录/权限或差分平台，不新增测试或实现任务文件。第 1–3 章与总计划只读；本章已明确的 range 基础契约、name/regtype、unknown 参数定型、缺工厂、完整多态、非 C locale 和扩展调用形态交后续依赖处理，不递归补建。文档验收不启动数据库、外部客户端、Cargo/Go 或下推性能测试，来源蓝图状态不自动改变。

### 验收

#### 三层身份与契约模板

文档验收要求三层身份可从一个标量候选追踪、模板全部字段有来源或明确缺口、五个全局小节齐全。下表为**待新增**用例族和候选能力记录，沿第 1 章格式落地，本轮没有新增/执行测试。每行 PG、MySQL 分别核对 AsterSQL 入口与自身参考端；成功均一列 v、无副作用、SQLSTATE/错误位置不适用，NULL 项按逻辑可空性记录；拒绝项无成功值/NULL/结果载荷，不能作为函数成功用例。

| 场景 ID 与区分目的 | PG 输入与目标 | MySQL 自身输入/可见性与目标 | 来源、缺口与证据等级 |
| --- | --- | --- | --- |
| FUNC-ID-ABS-SIGNATURE：同名不同签名 | 分别 `SELECT abs(CAST(-1 AS int4)) AS v`、`SELECT abs(CAST(-1 AS int8)) AS v`，均一行非 NULL 的 1；可见内建 abs，分别 23/4/-1、20/8/-1，不合并为同一签名。另查 pg_catalog.abs 的对应限定调用，目标同上。 | `SELECT ABS(CAST(-1 AS SIGNED)) AS v`，函数可见、返回非 NULL 的 1/BIGINT；PG int4/int8 语法不直接发送 MySQL。其自身 flags/length/decimals 按模板补证。 | PG 官方签名/类型目录；MySQL Go 工厂及 TestAbs。仅目标与静态来源，双方言名称/执行/元数据均未验证。 |
| FUNC-ID-ABS-SAME-VALUE：同值不同返回类型 | 沿用上行两条 SQL，数值均 1 仍须分别保持 int4/int8；不得从值或 payload 文本“1”合并 OID。 | 独立比较上行 SIGNED 与 `SELECT ABS(CAST(-1 AS DECIMAL(8,0))) AS v`，均精确值 1，返回 BIGINT 与 DECIMAL 身份不同；返回精度/scale/wire 字段另采证，不要求与 PG OID 相等。 | PG pg_proc.dat、MySQL Go Int/Decimal 分支；Rust 数学测试只证局部意图，实际两入口未验证。 |
| FUNC-ID-ABS-TYPED-NULL：NULL 不是未知身份 | `SELECT abs(CAST(NULL AS int4)) AS v` 与 int8 变体，均一行 SQL NULL，分别 23/4/-1、20/8/-1；扩展路径 `SELECT abs($1::int4) AS v`，Parse 参数 OID 23，绑定 -1/NULL/1 的三个新 portal，值为 1/NULL/1，结果描述始终 23/4/-1。 | `SELECT ABS(CAST(NULL AS SIGNED)) AS v`，函数可见、SQL NULL/BIGINT；MySQL Prepare `SELECT ABS(CAST(? AS SIGNED)) AS v` 独立重绑同组值，保持自身列定义。未定型 NULL、字符串 NULL 与空载荷不替代 typed NULL。 | strict 默认属性、Go nil 断言及第 3 章 META 契约；通用类型回退 text 不符合目标。两入口 NULL/准备/协议连续性均未验证。 |
| FUNC-ID-ABS-ERROR：签名与域检查分开 | `SELECT abs('-2147483648'::int4) AS v` 目标 22003，无临时 int8 提升；`SELECT abs() AS v` 目标 42883，无匹配签名，不能因 abs 名称存在成功。错误字段沿模板，确切阶段/P 待取证。 | `SELECT ABS(CAST(-2147483648 AS SIGNED)) AS v` 在 BIGINT 域内目标 2147483648，不能套 PG int4 溢出；`SELECT ABS() AS v` 拒绝错误参数数目，MySQL 精确码/位置待参考采集，无成功结果。 | PG int4abs/签名及错误分类；MySQL Go Int 工厂和 i64 局部算法支持域内目标，精确诊断仍缺证。未做参考端实测，不将共享内核错误字符串充当 PG SQLSTATE。 |

以上各成功场景 PG 同时定义简单 Q 和扩展 Parse/statement Describe/Bind/portal Describe/Execute 两路径；结果格式覆盖 text/binary，statement 格式 0、portal 按 Bind，OID/typlen/typmod 与 NULL 不随格式、重绑值或首行变化，Execute 只比对已描述类型与载荷，不要求它再发 RowDescription。另给首行 NULL 后接 1 的夹具 `SELECT abs(v) AS v FROM (VALUES (1,NULL::int4),(2,1::int4)) AS t(ord,v) ORDER BY ord`，目标两行 NULL/1；全 NULL 使用上表单行 typed NULL，零行使用 `SELECT abs(CAST(NULL AS int4)) AS v WHERE false`。三者描述仍 23/4/-1，行数分别为二/一/零。MySQL 使用自身 Query/Prepare、结果列定义和参数格式做对应检查，PG SQL/格式不直接套用。函数别名、两个 PG 会话及同时存在的 MySQL 连接需交错观测可见性与类型隔离；没有默认批准任何跨方言别名。此处均是未采集的验收目标，第 3 章类型/codec 与工作流 7 生命周期负责落实。

人工交接检查须将值/NULL、类型、元数据、错误、可见性和证据等级逐格对应；存在待定义字段时，文档可交接但相应行为验收不可记通过。结构、路径与 diff 检查只能证明文档完整性，不证明 SQL 正确、兼容或性能。

#### 注册工厂与方言可见性

以下四族为待新增验收目标。双方言使用独立连接，参考端保持相同无用户/扩展同名函数的前提；每行分别比较 AsterSQL 与所属方言参考端。错误项无成功值、NULL、结果类型或元数据，不能以一行 NULL 充当拒绝。

| 场景 ID | PG 入口输入、可见性与预期 | MySQL 入口输入、可见性与预期 | 来源与当前证据等级 |
| --- | --- | --- | --- |
| FUNC-REG-PG-ONLY | `SELECT pg_catalog.date_trunc('hour'::text, TIMESTAMP '2001-02-16 20:38:40') AS v`；可见，非 NULL 的 2001-02-16 20:00:00 / timestamp without time zone，1114/8/-1。非限定 date_trunc 同样可见。成功 SQLSTATE 不适用。 | `SELECT date_trunc('hour', CAST('2001-02-16 20:38:40' AS DATETIME)) AS v`；本目标集合不可见，在名称/签名解析拒绝，MySQL 精确错误码/SQLSTATE 待参考采集。用自身合法参数语法，避免 PG cast 语法错误掩盖函数门禁。 | PG 官方 §9.9/类型目录给语义目标；MySQL 隐藏为方言设计边界，参考端仍待核实；当前没有 PG 工厂或两入口通过证据。 |
| FUNC-REG-MYSQL-ONLY | `SELECT ifnull(NULL::text, 'fallback'::text) AS v`；PG 目标不可见，42883；不得自动改写成 COALESCE 或泄露 MySQL 工厂。 | `SELECT IFNULL(CAST(NULL AS CHAR), CAST('fallback' AS CHAR)) AS v`；可见，非 NULL fallback/字符串；另以第二参数 `CAST(NULL AS CHAR)` 得 SQL NULL/字符串，类型描述不得由首行或空值改变；wire 精确字段待采集，成功 SQLSTATE 不适用。 | IFNULL 核心工厂条目及独立 `migrated_string_control_and_like_factories_match_go_runtime` 给局部构造/值断言意图；typed CHAR、全 NULL、PG 拒绝与两入口元数据未实测。 |
| FUNC-REG-UNKNOWN | `SELECT astersql_contract_missing_fn_20261023(1::int4) AS v`；确认名称不在内建/用户/扩展候选，目标 42883，名称解析拒绝。 | `SELECT astersql_contract_missing_fn_20261023(CAST(1 AS SIGNED)) AS v`；同样不可见并拒绝；固定已选库，错误码/SQLSTATE/消息待参考采集。 | `newFunctionImpl` 的缺名分支及注册表测试负例是静态证据，不证明 PG SQLSTATE 已映射；两入口拒绝未实测。 |
| FUNC-REG-UNLINKED | 受控夹具保留可见 `abs(int4)` 类并确保工厂缺失，`SELECT abs(1::int4) AS v`；名称/参数已选中仍拒绝，目标 0A000（AsterSQL 实现缺口设计），不得误报 42883 或返回 NULL。正常 PG 参考端应为非 NULL 1/int4，23/4/-1；拒绝与成功不能算差分等价。 | 独立夹具保留 ABS 类/缺工厂，`SELECT ABS(CAST(1 AS SIGNED)) AS v`；构造拒绝，MySQL 实现缺口码/SQLSTATE 待定义；正常参考端目标非 NULL 1/BIGINT，元数据沿首块补证。 | `invoke_factory` 缺工厂错误及 abs 规格/核心表差异是夹具依据；动态注册须在后续测试中隔离并恢复，不据当前表断言生产 abs 必失败。此为工程拒绝用例，不得登记为功能支持或普通 PG 等价通过。 |

执行交接：PG 成功项覆盖 Q 与 Parse/Describe/Bind/Execute 的 text/binary 格式，返回描述依第 3 章固定；错误项分别采集名称/类型分析或构造失败阶段与协议 ErrorResponse，不臆定现有阶段。MySQL 覆盖 Query/Prepare 及其自身列定义。交错两个 PG 会话和一个 MySQL 连接，重用名称并重复准备/执行，验证可见性及缓存不串方言；进程全局注册变动不得用作切换连接方言。缺工厂夹具须隔离全局表，不能污染其他会话或测试。文档逐字段审阅通过只表示目标及缺口可交接，缺失诊断、typed NULL 或 wire 证据的行为仍未验收。

#### 重载与隐式转换选择

下表为**待新增** FUNC-RES 用例族，全部运行未验证。PG 同时采集 simple Q 与 Parse/statement Describe/Bind/portal Describe/Execute，双方言各对自身参考端；成功核对一行值/NULL、类型与可见性，失败核对 SQLSTATE 及首块错误字段。MySQL 旁路是隔离断言，不构造 PG 参数语法或虚构 MySQL 重载选择；其精确元数据缺口必须补证后才能升级证据。

| 五组目标用例 | PG 输入、选择、值/NULL/类型/错误 | MySQL 入口与证据边界 |
| --- | --- | --- |
| FUNC-RES-EXACT：精确签名优先 | `SELECT pg_catalog.abs((-1)::int4) AS v` 与 int8 版本分别精确绑定对应输入，均非 NULL 的 1，结果 23/4/-1 与 20/8/-1。增加 `SELECT pg_catalog.round(4::int4,4::int4) AS v`：I 资格转 numeric，结果 4.0000、1700/-1/-1，证明无精确签名时合法隐式转换；成功无错误。 | `SELECT ABS(CAST(-1 AS SIGNED)) AS v` 目标非 NULL 的 1/BIGINT，沿首块元数据取证；MySQL 不接受 PG 的 int4/int8 签名作为自身断言。PG round 例直接来自 §10.3，双方言全链未验证。 |
| FUNC-RES-UNKNOWN：unknown 定型 | `SELECT pg_catalog.substr('1234',3::int4) AS v` 将 unknown 按 string 类选择 text 输入，结果非 NULL 的 '34'，25/-1/-1。扩展正例 `SELECT pg_catalog.abs($1::int4) AS v` 用省略/0 OID 推定参数 23；绑定 -1、2、NULL 分别得 1、2、NULL，结果始终 23/4/-1，不能由 Bind 猜测重载。 | `SELECT SUBSTR('1234',3) AS v` 目标非 NULL 的 '34'/字符串；其返回类型码和字符元数据待自身参考采集，不把 PG unknown 规则搬入 MySQL。当前 PG 普通省略 OID 测试断言 0A000，目标推断未实现的部分须单列缺口。 |
| FUNC-RES-NULL：NULL 参数 | `SELECT pg_catalog.abs(NULL::int4) AS v`、int8 版本各得一行 SQL NULL、分别 23/4/-1 与 20/8/-1；NULL 不改签名。未定型 NULL 的歧义见 AMBIG，不能一律先定 text；`abs(NULL::text)` 目标 42883，没有 NULL 成功结果。 | `SELECT ABS(CAST(NULL AS SIGNED)) AS v` 目标 SQL NULL/BIGINT，沿 MySQL NULL 契约；不能把两入口的物理空值表示当作类型相同证据。PG typed NULL 全链未验证。 |
| FUNC-RES-NO-MATCH：无匹配 | `SELECT pg_catalog.abs('1'::text) AS v` 目标 42883，text 已定型且无到数值的 I 资格，不尝试读值后选择；`SELECT pg_catalog.abs() AS v` 同码但因为参数数目无匹配。两者无成功结果；与已选签名后的非法输入错误分开记录。 | `SELECT ABS() AS v` 拒绝参数数目，MySQL 精确码/消息待采集；合法旁路 `ABS(CAST(-1 AS SIGNED))` 仍为 1/BIGINT。不能把 PG text 拒绝目标推广为 MySQL 文本数值转换禁令。 |
| FUNC-RES-AMBIG：歧义 | 待新增解析夹具只暴露同 schema 的 `f(int4)→int4`、`f(int8)→int8`，无默认/可变/其他候选、两型都非 numeric 类优选。对 `f('1')`、`f(NULL)` 无精确或唯一优选目标 42725；显式 `f(1::int4)` 唯一绑定 int4，分析结果 23/4/-1（夹具不执行，无值/NULL 断言）。前两项错误而非任取首候选，按 §10.3 推导、待参考解析取证。 | 夹具仅用于 PG 解析，无 MySQL 可见函数 f，不能在 MySQL 注入同名生产函数。旁路 `SELECT ABS(CAST(-1 AS SIGNED)) AS v` 仍为 1/BIGINT；验证 PG 夹具和连接不改变 MySQL 可见集合。该旁路及 PG 歧义均未运行。 |

成功项的 Prepare/Describe 与 Execute 类型必须一致：显式参数 OID 与上下文兼容时保留其源身份，未知参数由分析固定；statement Describe 返回 ParameterDescription 与结果类型，portal Describe 只改变格式字段，Execute 按固定身份编码且不额外发送 RowDescription。新 Bind 换值/NULL、text/binary 不改变签名或结果描述，失败项不能发送伪成功描述/数据；binary 缺 codec 须如实登记能力缺口。MySQL Prepare/Execute 按其参数约束与元数据独立核对，不能套 PG OID。夹具 AMBIG 只验解析选择/拒绝，不宣称执行成功；可变参数与默认参数的未来聚焦解析验收还须覆盖展开/显式数组、同签名优先及默认歧义，不能以五组内建例证明这些能力已实现。

#### 等价共享与别名门槛

以下 FUNC-EQUIV 五组均为**待新增、未运行**的对照；每条分别比较 PG/AsterSQL PG 与 MySQL/AsterSQL MySQL，不要求两种参考端类型相等。两入口的 COALESCE/NULLIF 均要求在自身语法中可见，PG IFNULL 仍按前块拒绝。成功均单列 v、无错误，NULL 与类型独立检查；错误项无成功行或 NULL 载荷。

| 场景族 | PG 输入及目标值/NULL/类型/错误 | MySQL 自身输入及目标/缺口 |
| --- | --- | --- |
| FUNC-EQUIV-VALUE：普通值 | `SELECT COALESCE(NULL::int4,7::int4,9::int4) AS v`→7；`SELECT NULLIF(7::int4,7::int4) AS v`→NULL；第二参数换 9→7，均 int4。另 `NULLIF(7::int4,NULL::int4)`→7，`NULLIF(NULL::int4,7::int4)`→NULL，均 int4。 | 对应使用 `CAST(... AS SIGNED)`（NULL 同样显式 cast），目标值依次 7/NULL/7/7/NULL，目标 BIGINT；精确 flags、可空性/元数据由参考端补证。不能直接发送 PG ::int4。 |
| FUNC-EQUIV-ALL-NULL：全 NULL | `SELECT COALESCE(NULL::int4,NULL::int4) AS v` 与 `SELECT NULLIF(NULL::int4,NULL::int4) AS v`→SQL NULL/int4；另 `SELECT COALESCE(NULL,NULL) AS v`→SQL NULL/text，不能按本地 Null/binary 默认值回退。 | 两个 typed SIGNED 版本→SQL NULL/BIGINT 目标；未定型 `SELECT COALESCE(NULL,NULL) AS v` 保留其自身 NULL 类型/列定义，确切 MySQL wire 待采，不能强定 PG text。 |
| FUNC-EQUIV-MIXED：混合类型 | `SELECT COALESCE(NULL::int4,2.2::numeric) AS v`→2.2/numeric；`SELECT NULLIF(1::int4,2.2::numeric) AS v`→1/numeric，体现比较后首参数提升。`SELECT COALESCE(1::int4,'x'::text) AS v` 即使首项非 NULL 也应在分析拒绝，目标 42804，无值。 | `SELECT COALESCE(CAST(NULL AS SIGNED),CAST(2.2 AS DECIMAL(2,1))) AS v`→2.2/DECIMAL 目标；`SELECT NULLIF(CAST(1 AS SIGNED),CAST(2.2 AS DECIMAL(2,1))) AS v`→1，第一参数整数返回身份作为待参考核实目标；精度/scale/wire 缺证。`COALESCE(CAST(1 AS SIGNED),CAST('x' AS CHAR))` 保留 MySQL 自身混型规则，预计取首值但类型待采，不套 PG 42804；本组不批准共享。 |
| FUNC-EQUIV-UNREACHED-ERROR：未选分支与必需比较 | 待新增普通关系夹具 `contract_equiv_input(a int4,z int4)` 含一行 (7,0)，无将 z 固化为常量的约束：`SELECT COALESCE(a,10/z) AS v FROM contract_equiv_input`→7/int4，无除零错误；改 a 为 NULL 后应为 22012。`SELECT NULLIF(a,10/z) AS v FROM contract_equiv_input` 在 a=7 时也须计算比较所需右参，目标 22012。分析错误仍不能被短路；常量 `1/0` 不作为运行期跳过的唯一证据，实际失败阶段须采集。 | 自身普通关系夹具采用 BIGINT 列，COALESCE(a,10/z) 在 (7,0) 应跳过后参并取 7，结果类型由自身 `/` 与聚合规则决定，非自动 BIGINT。a=NULL 及 NULLIF 右参的除零值/警告/错误须固定 sql_mode 按参考端采集，不承诺与 PG 22012 同义。另沿已读局部 Datum::Error 夹具覆盖未达/必达错误，但局部通过不能代替 SQL 证据。 |
| FUNC-EQUIV-COLLATION：字符串比较与结果属性 | `SELECT COALESCE(NULL::text,'A'::text COLLATE "C") AS v`→非 NULL 'A'/text，输出 C；`SELECT NULLIF('A'::text COLLATE "C",'a'::text COLLATE "C") AS v`→非 NULL 'A'/text；把第二值换 'A'→NULL/text。负例 `SELECT COALESCE('A'::text COLLATE "C",'a'::text COLLATE "POSIX") AS v` 显式冲突目标 42P21，不能因为首项非 NULL 忽略分析错误。 | `SELECT COALESCE(CAST(NULL AS CHAR CHARACTER SET utf8mb4),_utf8mb4'A' COLLATE utf8mb4_bin) AS v`→'A'/字符串、utf8mb4_bin；`SELECT NULLIF(_utf8mb4'A' COLLATE utf8mb4_bin,_utf8mb4'a' COLLATE utf8mb4_bin) AS v`→'A'，第二值换 'A'→NULL。不把 PG C/POSIX 名称发给 MySQL；本地比较上下文快照断言不能证明上述 SQL/输出 collation，精确 wire 和冲突诊断仍待采。 |

每个成功项覆盖 PG Q 与 Parse/statement Describe/Bind/portal Describe/Execute、text/binary；以带显式 cast 的参数重绑值/NULL，类型与 collation 必须由分析固定，不能随选中分支变化。全 NULL 仍有固定描述；结果逻辑可空性单独登记。MySQL 覆盖 Query/Prepare 及自身列定义，采集当前 sql_mode/字符设置和警告；两个 PG 连接与 MySQL 连接交错验证名称、类型和缓存隔离。所有行均只有官方规则或局部静态断言意图，没有两入口运行证据，因此本轮**批准共享/别名数量为 0**。结构检查不能升级任一场景为功能通过。

#### 同名异义分流

下表 FUNC-SPLIT 四族均为**待新增、未运行**目标；每条独立比较参考 PG/AsterSQL PG 和参考 MySQL/AsterSQL MySQL。两入口名称均目标可见，参数/类型负例仍须在正确阶段拒绝；成功均单列 v、无错误，非 NULL 空串与 SQL NULL 分开断言，wire 使用本块模板。MySQL 的“字符串”指自身结果类型，精确元数据待补，不能当作 PG text。

| 场景族 | PG 输入与值/NULL/类型/错误目标 | MySQL 自身输入与目标、证据边界 |
| --- | --- | --- |
| FUNC-SPLIT-NULL | `SELECT concat('a'::text,NULL::text,'b'::text) AS v`→非 NULL 'ab'/text；`concat(NULL::text,NULL::text)`→非 NULL 空串/text。`SELECT GREATEST(NULL::int4,7::int4) AS v` 与 LEAST 版本→7/int4；全换 NULL::int4→NULL/int4；另 `GREATEST(NULL,NULL)`→NULL/text。 | `SELECT CONCAT(CAST('a' AS CHAR),CAST(NULL AS CHAR),CAST('b' AS CHAR)) AS v`→NULL/字符串；全 NULL CHAR 同样 NULL。`SELECT GREATEST(CAST(NULL AS SIGNED),CAST(7 AS SIGNED)) AS v` 及 LEAST→NULL/BIGINT 目标，全 NULL SIGNED 同样 NULL。Rust 局部 NULL 断言只支持 MySQL 算法意图，不证明 SQL/typed NULL/wire。 |
| FUNC-SPLIT-MIXED | `SELECT concat('v='::text,2::int4) AS v`→非 NULL 'v=2'/text；`SELECT GREATEST(2::int4,10.5::numeric) AS v`→10.5/numeric，LEAST→2/numeric。`SELECT GREATEST(2::int4,'10'::text) AS v` 及 LEAST→分析拒绝 42804，无值；typed text 不能被当作 unknown 数字串。 | `SELECT CONCAT(CAST('v=' AS CHAR),CAST(2 AS SIGNED)) AS v`→'v=2'/字符串；`SELECT GREATEST(CAST(2 AS SIGNED),CAST(10.5 AS DECIMAL(3,1))) AS v`→10.5/DECIMAL，LEAST→2/DECIMAL 目标。`SELECT GREATEST(CAST(2 AS SIGNED),CAST('10' AS CHAR)) AS v` 及 LEAST 的目标按自身字符串比较为 '2'/'10'，返回字符串；混型精确类型/precision/scale/wire 待采，Go 工厂/混型测试只有静态意图，不套 PG 拒绝。 |
| FUNC-SPLIT-COLLATION | `SELECT GREATEST('Z'::text COLLATE "C",'a'::text COLLATE "C") AS v`→'a'/text，LEAST→'Z'/text，结果 collation C。`SELECT concat('Z'::text COLLATE "C",'a'::text COLLATE "C") AS v`→'Za'/text、C，证明拼接不排序。极值第二参数显式换 COLLATE "POSIX"→冲突拒绝目标 42P21，无值。 | `SELECT GREATEST(_utf8mb4'Z' COLLATE utf8mb4_bin,_utf8mb4'a' COLLATE utf8mb4_bin) AS v`→'a'/字符串，LEAST→'Z'；对应 CONCAT→'Za'，三者结果 collation 目标 utf8mb4_bin。固定 ASCII/相同显式规则，此域值可相同但不能由此批准全域等价；不把 C/POSIX 名称发到 MySQL。冲突诊断/精确 wire 由自身参考端补证。 |
| FUNC-SPLIT-ARITY | `SELECT concat('x'::text) AS v`→'x'/text；`SELECT concat() AS v`→无匹配函数目标 42883。`SELECT GREATEST(7::int4) AS v` 与 LEAST→7/int4；`SELECT GREATEST() AS v` 与 LEAST→语法拒绝目标 42601，无值，不能套用普通函数 42883。 | `SELECT CONCAT(CAST('x' AS CHAR)) AS v`→'x'/字符串，`SELECT CONCAT() AS v` 拒绝参数个数；`SELECT GREATEST(CAST(7 AS SIGNED)) AS v` 和 LEAST 均拒绝，不能因局部 helper 接受单值而开放；两 SIGNED 参数 (7,9) 正例→9/7、BIGINT。零参数极值同样拒绝。MySQL 错误码/SQLSTATE/消息待参考采集，不复用 PG 42601/42883。 |

成功项覆盖 PG simple Q 与 Parse/statement Describe/Bind/portal Describe/Execute 的 text/binary，并使用显式 cast 的参数重绑普通值/NULL，参数与结果身份在分析固定；零行或全 NULL 仍有类型描述。MySQL 覆盖 Query/Prepare/Execute 及自身列定义、警告/会话设置。交错两个 PG 连接和一个 MySQL 连接，以相同名称、不同 NULL/参数策略验证缓存与可见性隔离；不能修改进程全局注册模拟连接方言。拒绝项采实际阶段和完整错误字段，无伪成功行；缺工厂沿注册块单列工程缺口，不能冒充参考端语义错误。四组仅有官方目标/静态证据，未批准任何共享别名、下推或功能支持。

#### 时间截断与提取

以下 FUNC-TEMP 五组全部为**待新增、未运行**用例。PG 名称/签名目标可见，合法项成功无错误、单列 AS v；除明确 NULL 项外均非 NULL，结果类型/wire 使用本块模板。每条独立比较参考 PG 与 AsterSQL PG，MySQL 旁路对其自身参考端，不要求跨方言同型。固定构建、编码、p、时区与 tzdata，不能把这些文档预期记成差分结果。

| 五组场景 | PG 输入与目标值/NULL/类型/错误 | MySQL 入口与证据等级 |
| --- | --- | --- |
| FUNC-TEMP-DAY：跨日与精度 | `SELECT date_trunc('day',TIMESTAMP '2024-01-02 00:00:00.123456') AS v`→2024-01-02 00:00:00/timestamp；换前日 23:59:59.999999→2024-01-01 00:00:00；`date_trunc('week',TIMESTAMP '2021-01-01 12:00:00')`→2020-12-28 00:00:00。`date_trunc('milliseconds',TIMESTAMP '2024-01-02 00:00:40.123456')`→00:00:40.123（日期不变）。对该原输入 `EXTRACT(SECOND FROM ...)`→40.123456/numeric，`date_part('second',...)`→40.123456/float8（按 float8 正确舍入及编码核对）；`EXTRACT(MICROSECONDS FROM ...)`→40123456/numeric。 | `SELECT EXTRACT(SECOND FROM CAST('2024-01-02 00:00:40.123456' AS DATETIME(6))) AS v` 保留自身整数 40 目标；MICROSECOND 版本目标 123456，不能套 PG 整秒累计单位。MySQL 精确列定义及两入口未采；静态正式工厂为整数分支，不能证明上述 SQL 通过。 |
| FUNC-TEMP-ZONE：时区变化 | `SELECT date_trunc('day',TIMESTAMPTZ '2024-01-02 01:30:00+00') AS v` 在 UTC→2024-01-02 00:00:00+00，在 America/New_York→2024-01-01 00:00:00-05（05:00Z），均 timestamptz。固定 UTC 显示，三参 zone='America/New_York'→2024-01-01 05:00:00+00。同一输入 `EXTRACT(HOUR FROM ...)` 在两会话分别 1/20、numeric，date_part('hour',...) 同值/float8；timestamp '2024-01-02 01:30:00' 的 day 截断在两区域均 2024-01-02 00:00:00/timestamp，不能添造瞬时。 | PG 两名称在 MySQL 隐藏；以 `SELECT date_trunc('day',CAST('2024-01-02 01:30:00' AS DATETIME)) AS v`、同参数 date_part 验证名称拒绝（错误码/消息待采）。合法旁路 `EXTRACT(HOUR FROM CAST('2024-01-02 01:30:00' AS DATETIME))`→1/自身整数目标，不复制 PG timestamptz。区域转换独立断言仅静态意图。 |
| FUNC-TEMP-DST：DST 边界 | TimeZone=UTC，`SELECT date_trunc('day',TIMESTAMPTZ '2024-03-10 12:00:00+00','America/New_York') AS v`→2024-03-10 05:00:00+00，次日同时间输入→2024-03-11 04:00:00+00，相邻桶相差 23h；秋季 2024-11-03 / 11-04 12:00+00 分别→11-03 04:00Z / 11-04 05:00Z，相差 25h，均 timestamptz。TimeZone=America/New_York，显式瞬时 2024-11-03 05:30Z / 06:30Z 的 HOUR 提取均 1/numeric，timezone 分别 -14400/-18000；date_part 同字段同数值/float8。避免模糊墙钟输入掩盖偏移差异。 | 继续拒绝 PG 专属名称；MySQL 自身 `EXTRACT(HOUR FROM CAST('2024-11-03 01:30:00' AS DATETIME))`→1/整数目标，不声称其保存两个瞬时。已有 Amsterdam 缺口/重叠转换断言不等于 PG 截断。DST 数值为规则推导，参考端、tzdata 与双方言运行证据均待采。 |
| FUNC-TEMP-INTERVAL：interval 字段 | `SELECT date_trunc('hour',INTERVAL '2 years 3 months 4 days 05:06:07.123456') AS v`→月分量 27、日 4、时间 05:00:00/interval。对原输入依次 `EXTRACT(YEAR/MONTH/DAY/HOUR/SECOND FROM ...)`（每字段单独一条 SQL）→2/3/4/5/7.123456、numeric；date_part 对应文本字段→同数值/float8。`EXTRACT(MINUTE FROM INTERVAL '80 minutes')`→20/numeric；`date_trunc('hour',INTERVAL '-1 day -02:03:04')`→日 -1、时间 -02:00:00/interval，不能按总秒数向负无穷取整或把月折 30 天。 | 不发送 PG interval 语法；`SELECT EXTRACT(HOUR FROM CAST('27:06:07' AS TIME)) AS v`→27/自身整数目标，保留 MySQL 长 duration，不能充当 interval 日/月字段证据。正式工厂 Duration 分支只有局部意图，SQL/元数据待采。 |
| FUNC-TEMP-NULL-INVALID：NULL/非法单位 | 对每个 T=timestamp/timestamptz/interval，`SELECT date_trunc('day',NULL::T) AS v`→NULL/T；`date_part('day',NULL::T)`→NULL/float8；`EXTRACT(DAY FROM NULL::T)`→NULL/numeric。`date_trunc(NULL::text,TIMESTAMP '2024-01-01')`→NULL/timestamp，`date_trunc('day',TIMESTAMPTZ '2024-01-01 00:00:00+00',NULL::text)`→NULL/timestamptz。非 NULL 有效源上 date_trunc/date_part 的字段换 'aster_bad_unit'→22023；`EXTRACT('aster_bad_unit' FROM TIMESTAMP '2024-01-01')`→22023；`date_trunc('week',INTERVAL '1 month')`、`EXTRACT(TIMEZONE FROM TIMESTAMP '2024-01-01')` 及 date_part('timezone',同 timestamp)→0A000。错误无成功行/NULL 载荷；NULL 与非法字段混合不作为错误阶段的唯一证据。 | `SELECT EXTRACT(DAY FROM CAST(NULL AS DATETIME)) AS v`→NULL/自身整数目标，不能变成 PG numeric。非法单位在 MySQL 依自身语法/签名拒绝，错误阶段、码/SQLSTATE 与警告待采，不能照搬 PG 22023。NULL 正式求值分支已静态读到，typed NULL 与双方言协议未验证。 |

所有 PG 成功项覆盖 simple Q 与 Parse/statement Describe/Bind/portal Describe/Execute、text/binary 参数到 text/binary 结果；源参数使用显式 cast 的 $1::T 重绑值/NULL；date_trunc/date_part 的字段参数可用 $2::text、三参 zone 参数可用 $3::text，EXTRACT 的字段保留在语法树中而不以 Bind 替换，分析固定 T 和返回身份，Bind 不换重载。statement Describe 和 portal Describe 的格式差异沿首块，Execute 不重发 RowDescription；NULL 或零行仍有固定类型描述。时区变化用两个 PG 连接交错，并在同一已准备语句执行前改变 TimeZone，区分 stable 执行上下文与三参指定区域；显式区域结果瞬时不变但文本显示随会话变化。MySQL Query/Prepare/Execute 独立采 code/flags/charset/length/decimals、sql_mode 与时区，验证 PG 名称隐藏及 EXTRACT 不被全局注册切换污染。错误采分析/折叠/执行实际阶段和完整错误字段；缺 codec/工厂记工程缺口，不冒充参考端语义拒绝。五组及官方/源码证据不批准别名、下推或功能支持。

#### 格式化函数契约

以下 FUNC-FMT 五组均为**待新增、未运行**目标，逐条分别对照参考 PG/AsterSQL PG 与参考 MySQL/AsterSQL MySQL。PG 成功项统一单列 AS v、text/25/-1/-1、无错误，除明确 NULL 项外非 NULL；结果空格和编码字节精确比较，不用 trim 或近似数值比较掩盖格式差异。固定 UTF-8，基线 lc_time/lc_numeric/lc_monetary=C、TimeZone=UTC。

| 五组场景 | PG 输入与预期值/NULL/类型/错误 | MySQL 入口、自身旁路与证据限制 |
| --- | --- | --- |
| FUNC-FMT-TIME：时间格式 | `SELECT to_char(TIMESTAMP '2024-01-02 17:03:04.123456','YYYY-MM-DD HH24:MI:SS.US') AS v`→'2024-01-02 17:03:04.123456'；`to_char(TIMESTAMP '2021-01-01','IYYY-IW')`→'2020-53'；`to_char(INTERVAL '36 hours','HH24:MI:SS')`→'36:00:00'，不能按日内时间取模。 | to_char 在 MySQL 拒绝；合法旁路 `SELECT DATE_FORMAT(CAST('2024-01-02 17:03:04.123456' AS DATETIME(6)),'%Y-%m-%d %H:%i:%s.%f') AS v`→相同可见文本/自身字符串目标；不发送 PG interval 语法，不由本例批准别名，精确列定义未采。 |
| FUNC-FMT-NUM：数值格式 | `SELECT to_char(148.5::numeric,'FM999.990') AS v`→'148.500'；`to_char(-485::int4,'999MI')`→'485-'；`to_char(12::int8,'FM000')`→'012'；分别以 `125.5::float4`、`125.5::float8` 和 'FM999.0'→'125.5'，不把浮点输入先转任意精度数值而抹去源身份。非法 `to_char(12::numeric,'99.9V99')`→42601，无成功行，阶段待采。 | to_char 均隐藏；合法独立数值旁路 `SELECT FORMAT(148.5,3,'en_US') AS v`→'148.500'/自身字符串目标。FORMAT 的 decimals/locale 参数不等同 PG 模板；PG V/小数点负例不发送给 FORMAT，MySQL 精确元数据/警告未采。 |
| FUNC-FMT-TEMPLATE：字面量/修饰符 | `SELECT to_char(TIMESTAMP '2024-01-02','"Year="YYYY FMDD DDTH') AS v`→'Year=2024 2 02ND'，证明引号字面量、FM 仅下一时间项、TH；同源模板 '%Y'→'%4'，不能按 DATE_FORMAT %Y 输出 2024。`to_char(TIMESTAMP '2024-01-02','"YYYY"')`→'YYYY'。 | `SELECT DATE_FORMAT(CAST('2024-01-02' AS DATETIME),'%Y') AS v`→'2024'/自身字符串目标；同源模板 'YYYY'→'YYYY'。to_char 名称仍拒绝；反向 PG 调用 `DATE_FORMAT(TIMESTAMP '2024-01-02','%Y')` 目标 42883，禁止用模板翻译自动放开名称。 |
| FUNC-FMT-NULL：NULL/额外时间边界 | 对每个已列 T，`SELECT to_char(NULL::T,'YYYY') AS v`→NULL/text；`to_char(12::numeric,NULL::text)`、`to_char(TIMESTAMP '2024-01-02',NULL::text)`→NULL/text。`to_char(TIMESTAMP '2024-01-02','')` 与 `to_char(TIMESTAMP 'infinity','YYYY')`→NULL/text，和非 NULL 空串区分。`to_char('abc'::text,'YYYY'::text)`→42883，无值。 | `SELECT DATE_FORMAT(CAST(NULL AS DATETIME),'%Y') AS v` 及 `DATE_FORMAT(CAST('2024-01-02' AS DATETIME),CAST(NULL AS CHAR))`→NULL/自身字符串目标；同有效日期空模板→非 NULL 空串目标。MySQL 不接受 PG infinity 字面量作同义输入；to_char 仍隐藏，错误字段与 wire 待采。 |
| FUNC-FMT-CONTEXT：locale/时区 | `SELECT to_char(TIMESTAMPTZ '2024-01-02 01:30:00+00','YYYY-MM-DD HH24:MI TZH:TZM') AS v` 在 UTC→'2024-01-02 01:30 +00:00'，America/New_York→'2024-01-01 20:30 -05:00'；timestamp 同墙钟输入及同模板去掉 TZH:TZM 后两会话均 '2024-01-02 01:30'。C 下 `to_char(TIMESTAMP '2024-01-02','TMMonth')`→'January'，`to_char(1234.5::numeric,'FM9G999D0')`→'1,234.5'。另在参考端确认存在的德语 locale 下采同两条，目标为该 locale 的一月译名与分组/小数字符，精确 locale 名及字节待采；. / , 固定标点不随配置变。 | `SELECT DATE_FORMAT(CAST('2024-01-02 01:30:00' AS DATETIME),'%Y-%m-%d %H:%i') AS v`→'2024-01-02 01:30'/自身字符串目标，不能充当 timestamptz 瞬时/偏移证据；独立 `FORMAT(1234.5,1,'de_DE')`→'1.234,5' 目标，局部测试只证明算法意图。MySQL 自身字符/locale/时区配置与精确元数据另采，不把 PG lc_* 设置发给 MySQL。 |

每族另以合法 MySQL DATE_FORMAT 调用在 PG 入口验证名称拒绝 42883，以合法 PG to_char 调用在 MySQL 验证名称拒绝（自身错误码/SQLSTATE/消息待采），此为两入口可见性隔离目标。PG 成功项覆盖 Q 与 Parse/statement Describe/Bind/portal Describe/Execute，显式 `$1::T,$2::text` 绑定重绑值/NULL、text/binary 参数和结果，分析固定 T/text，Bind 不改重载；两个 Describe 的格式差异沿首块，Execute 不重发描述。零行/全 NULL 仍固定 text 身份。MySQL Query/Prepare/Execute 独立采列定义、警告、sql_mode 与字符设置。两 PG 连接和一 MySQL 连接交错执行；同一已准备语句在执行前变更 TimeZone/允许修改的 locale 配置，验证 stable 上下文及缓存隔离，并恢复配置。非 C locale 不可用须记录环境缺口，不能静默跳过后宣称全组通过。错误采实际阶段与完整字段，缺工厂/codec 单列工程缺口。当前有效运行用例数为 0，五族只是官方目标/静态意图，没有功能通过、别名或下推批准。

#### 数组函数契约

以下 FUNC-ARR 五组全部为**待新增、未运行**。表中 a 是输入替换记号，每个表达式独立使用 `SELECT <表达式> AS v`，不是新建变量/测试设施。除显式 SQL NULL 外结果非 NULL；合法项无错误，计数/追加类型与 wire 沿本块模板。MySQL 名称隔离使用其合法语法，避免 PG 数组语法先报错掩盖名称门禁。

| 五组场景 | PG 输入及预期值/NULL/类型/错误 | MySQL 入口与证据限制 |
| --- | --- | --- |
| FUNC-ARR-EMPTY：空数组 | a=`ARRAY[]::int4[]`；`array_length(a,1)`→SQL NULL/int4；`cardinality(a)`→0/int4；`array_append(a,7::int4)`→{7}/int4[]、shape=[1]、lower=[1]。 | 每组三函数名均以 `SELECT array_length(JSON_ARRAY(),1) AS v`、`SELECT cardinality(JSON_ARRAY()) AS v`、`SELECT array_append(JSON_ARRAY(),7) AS v` 验证隐藏/拒绝，自身错误码/SQLSTATE/消息待采；不能用 JSON 空数组代表 PG 空数组。 |
| FUNC-ARR-NULL-ARRAY：数组 SQL NULL | a=`NULL::int4[]`；`array_length(a,1)`、`cardinality(a)`→SQL NULL/int4；`array_length(ARRAY[1]::int4[],NULL::int4)`→SQL NULL/int4；`array_append(a,7::int4)`→{7}/int4[]；`array_append(a,NULL::int4)`→{NULL}/int4[]，后两者 shape=[1]、lower=[1]，不是外层 NULL。 | 三名称用同一合法参数骨架，将 JSON_ARRAY() 换成 NULL，仍拒绝，不能因 NULL 常量折叠绕过可见性；错误与元数据未采。 |
| FUNC-ARR-NULL-ELEMENT：NULL 元素与定型 | a=`ARRAY[1,NULL]::int4[]`；`array_length(a,1)`→2/int4，`cardinality(a)`→2/int4；`array_append(a,NULL::int4)`→{1,NULL,NULL}/int4[]、shape=[3]/lower=[1]。`array_append(ARRAY[1]::int4[],2::int8)`→{1,2}/int8[]、元素 OID 20、结果 OID 1016，不能沿用输入 1007。 | 合法骨架改用 JSON_ARRAY(1,NULL)，仍拒绝 PG 名称；JSON null 与 SQL 元素 NULL 无等价批准，提升例不要求 MySQL 提供 PG 类型语法或 OID。 |
| FUNC-ARR-MULTIDIM：多维长度 | a=`ARRAY[[1,2,3],[4,5,6]]::int4[]`；`array_length(a,1)`→2、维号 2→3、3/0/-1→SQL NULL，均 int4；`cardinality(a)`→6/int4；`array_append(a,7::int4)`→22000、无结果。形状 [2,3] 不把数组 OID 1007 改成另一“二维 OID”。 | 合法骨架改用 JSON_ARRAY(JSON_ARRAY(1,2,3),JSON_ARRAY(4,5,6))，三名仍拒绝；MySQL JSON 嵌套数组不作矩形 shape 或 PG 多维追加证据。 |
| FUNC-ARR-LOWER：非默认下界 | a=`'[0:1]={10,20}'::int4[]`；`array_length(a,1)`→2/int4，`cardinality(a)`→2/int4，`array_append(a,30::int4)`→[0:2]={10,20,30}/int4[]、shape=[3]、lower=[0]。下界换 -2 的 `a='[-2:-1]={10,20}'::int4[]` 追加后→[-2:0]={10,20,30}。`array_append('[2147483647:2147483647]={1}'::int4[],2::int4)`→22003、无结果，不能环绕索引。 | 沿 EMPTY 的合法骨架验证名称隐藏；MySQL 无本块 PG 下界载体，不伪造等价输入/返回 OID。拒绝实际阶段与字段未采，不将未实现形状误判为参考端成功。 |

每组分别对照 PG 参考端/AsterSQL PG；成功项覆盖 Q 与 Parse/statement Describe/Bind/portal Describe/Execute，显式 `$1::int4[]`、维号 `$2::int4` 或元素 `$2::int4`，提升例用 `$2::int8`。参数 text/binary × 结果 text/binary 四组合验证同一规范值、维度/下界/元素 NULL；重复 Bind 换值、空数组、数组 NULL 或 NULL 元素均不换已选签名/返回 OID。NULL 结果与零行仍有固定描述，两个 Describe 的格式差异沿首块，Execute 不重发描述；错误采实际阶段，不强制延迟到 Execute。MySQL 覆盖 Query/Prepare/Execute 的名称拒绝，并以既有合法 `SELECT 1 AS v` 作成功旁路，单独采自身列定义、警告和错误码/SQLSTATE，不引入新 JSON 函数验收。两个 PG 连接和一个 MySQL 连接交错验证可见性/缓存隔离。缺数组表示、codec、定型或工厂单列工程缺口，不当作参考 PG 目标错误；当前有效运行用例为 0，无功能通过证据。

#### 范围函数契约

以下四组均为**待新增、未运行**目标，每个表达式独立执行 `SELECT <表达式> AS v`。r 为表内输入替换记号，不是新增变量设施。PG 合法项可见且成功无错误，除显式 NULL 外为非 NULL；返回身份按本块模板，成功 SQLSTATE 不适用。

| 场景 | PG 输入与值/NULL/类型/错误目标 | MySQL 入口及证据限制 |
| --- | --- | --- |
| FUNC-RNG-EMPTY-NULL：空与 NULL | r=`'empty'::int4range`：lower(r)/upper(r)→SQL NULL/int4，isempty(r)→true/bool；r=`NULL::int4range` 三者均 SQL NULL，类型分别 int4/int4/bool。`int4range(4,4)`→empty/int4range；`int4range(4,4,'[]')`→[4,5)，非空。 | 合法语法 `isempty(NULL)`、`int4range(4,4)` 目标名称拒绝，MySQL 自身错误码/SQLSTATE 待采；不能把 PG cast 语法错误当名称门禁证据。两侧无当前运行记录。 |
| FUNC-RNG-UNBOUNDED：缺端点 | `int4range(NULL::int4,5)`→(,5)，lower→NULL/int4、upper→5/int4、isempty→false/bool；`int4range(2,NULL::int4)`→[2,)，lower→2、upper→NULL、isempty→false；`int4range(NULL::int4,NULL::int4,'[]')`→(,)，两提取 NULL、isempty=false。均为非 NULL range，缺侧自动排除边界，不是 subtype 的显式 infinity。 | `int4range(NULL,5)`/`numrange(NULL,5)` 仍隐藏；PG 无界域不映射为 MySQL 字符串 NULL 语义。成功旁路 `SELECT 1 AS v`→1/自身整数，精确列定义待采。 |
| FUNC-RNG-BOUNDS：开闭与非法构造 | 对 flags 四值 [)、(]、[]、()，`numrange(1::numeric,3::numeric,flags)` 输出相应边界，lower=1、upper=3/numeric、isempty=false；`numrange(4::numeric,4::numeric,'[]')` 为单点非空，其余三种同端点为空。`int4range(5,2)`→22000；`int4range(1,3,NULL::text)`→22000；flags='bad'→42601，均无成功行。 | `lower('AbC')`→'abc'、`upper('AbC')`→'ABC'/自身字符串为保留目标；`numrange(1,3,'[]')` 隐藏。字符串大小写结果不证明 PG range 提取；MySQL 自身元数据/错误字段未采。 |
| FUNC-RNG-CANON：离散规范化 | `int4range(1,4,'(]')`→[2,5)，lower=2、upper=5/int4；`int4range(2,5)` 得同一规范 range；`int4range(1,2,'()')`→empty。连续 `numrange(1::numeric,4::numeric,'(]')` 仍 (1,4]、提取 1/4/numeric。`int4range(1,2147483647,'[]')`→22003，不把溢出自动变无界或换 int8range。 | range 构造名称仍隐藏，lower/upper 的字符串签名保持可见；PG 专属输入不跨发到 MySQL 作同值对比。全部只具官方目标/静态入口证据。 |

每组独立对照参考 PG/AsterSQL PG，成功项覆盖 Q 与 Parse/statement Describe/Bind/portal Describe/Execute；用 `$1::int4range`、`$1::numrange` 或定型端点/flags 参数，text/binary 参数×结果四组合，重复 Bind 普通值、empty、无界、SQL NULL 不换已选签名和 OID。全 NULL/零行仍有固定描述，Execute 不重发描述；构造 NULL 端点不按 strict 提前吞掉。MySQL Query/Prepare/Execute 用合法骨架检查隐藏名称及保留字符串签名，采自身 code/flags/charset/length/decimals、警告和错误；两个 PG 连接与一个 MySQL 连接交错验证缓存/可见性隔离。codec、表示或工厂缺失单列工程缺口，不能将其记作目标拒绝通过；当前有效运行用例为 0。

#### 必需系统函数契约

以下五组为**待新增、未运行**目标；每项表达式独立 `SELECT <表达式> AS v`，类型及 wire 按本块模板。固定无同名用户函数、有效数据库 db_a/db_b、UTF-8；成功项除显式 schema NULL 外非 NULL，成功 SQLSTATE 不适用。

| 场景 | 输入及 PG 值/NULL/类型/错误目标 | 事实提供者、两入口与缺证边界 |
| --- | --- | --- |
| FUNC-SYS-SESSION：会话变化 | A/B 分别连接 db_a/db_b，current_database()→各库名/name；A 的 search_path 从 public 改 pg_catalog 再改空，current_schema() 依次 public、pg_catalog、SQL NULL/name，B 不变；RESET 回到夹具记录的默认值。Parse current_schema() 后 SET 再 Bind/Execute 应观察新值。 | 5/7 提供所属连接数据库与路径，6 提供有效 schema。MySQL DATABASE() 只反映自身库，PG SET 不改变它；同值不批准别名。现有局部测试只有 test 与 public/catalog/空路径，不证明双库、完整元数据或两入口隔离。 |
| FUNC-SYS-VISIBILITY：权限与可见性 | 参考夹具建存在的 s_hidden（无 USAGE）、s_ok（有 USAGE），SET search_path TO missing,s_hidden,s_ok；current_schema()→s_ok/name。路径只剩 missing,s_hidden→SQL NULL/name；隐式 pg_catalog 仍可解析限定系统函数。授予/撤销 USAGE 后在下一语句重新验证有效结果。 | 6 提供真实存在/授权事实，5 按当前角色构造有效路径；不能从函数名或路径文本伪造。当前 AsterSQL 有界路径拒绝任意 schema 是工程缺口，非 PG 目标；GRANT/角色基础设施属外部依赖。MySQL 不公开 current_schema() PG 签名，错误码/SQLSTATE 待采，不能拿 PG SET 语法错误作名称隔离证据。 |
| FUNC-SYS-TYPEOF：类型内省 | pg_typeof(1::int4)、pg_typeof(NULL::int4) 都返回 regtype 值 23，text 显示 integer；pg_typeof(NULL::text)→regtype 值 25，text 显示 text；均非 NULL，结果列 OID 恒为 2206。定型参数 `$1::int4` 重复 Bind 1/NULL 保持 integer；pg_typeof()→42883 无成功行。 | 第 3 章提供分析类型，6 提供真实类型目录与 regtype 显示，server 编码。MySQL 合法骨架 pg_typeof(1) 目标名称拒绝，自身诊断待采；不把内部 MySQL BIGINT 直接报告为 PG int4。typed NULL、参数、regtype codec 和完整工厂均无当前运行证据。 |
| FUNC-SYS-IDENTITY：连接身份 | 同时活动的 PG A/B 重复 pg_backend_pid() 各得稳定、非 NULL、正 int4 且两者不同；各值须等于本连接 BackendKeyData 的 backend ID，独立取消只作用于对应连接。断开后不要求 ID 永不重用。version() 返回真实服务版本说明/text，与所声明能力和启动参数一致，不逐字等同参考 PG 的机器/编译器串。 | 7 提供 backend ID/生命周期/取消关联，构建信息提供者和 7 提供版本策略；第 4 章只定义求值。MySQL CONNECTION_ID()/VERSION() 保持自身值和类型，不能截断 u64 到 int4 或互相覆盖版本缓存。现有启动 server_version 测试不是 SQL version/PID 证据；本块只定义交接，不运行取消平台。 |
| FUNC-SYS-ISOLATION：两入口隔离 | 交错 A/B 的 current_database/current_schema/pg_typeof/pg_backend_pid 与 MySQL M 的 DATABASE()/VERSION()/CONNECTION_ID()；PG version() 与 MySQL VERSION() 同名各选自身签名及返回元数据，重复准备/执行不得泄漏 A/B/M 状态。PG current_database(1)→42883，无成功行。 | 第 4 章选择签名与工厂，5/7 供连接事实，6 供目录；MySQL 中 current_database()/current_schema()/pg_backend_pid()/pg_typeof(1) 隐藏 PG 名称，分别用合法 MySQL 函数调用采自身拒绝，错误码/SQLSTATE 未采。PG DATABASE()/CONNECTION_ID() 不开放跨方言别名，名称无匹配目标 42883。没有双入口全链运行证据。 |

每组成功项分别在参考 PG 和 AsterSQL PG 采 Q、Parse/statement Describe、Bind/portal Describe、Execute，text/binary 结果及适用的参数格式；无参函数不虚构参数载荷，typeof 参数覆盖 text/binary × 结果格式。schema=NULL 和零行仍保留 OID/typlen/typmod；Execute 不重发描述。name/regtype codec 或工厂缺失单列依赖，不能退回 text 冒充通过。MySQL Query/Prepare/Execute 单独记录值/NULL、自身列定义、错误和警告，两个 PG 连接与一个 MySQL 连接交错采样。现有源码及断言意图与官方目标分别记录，有效运行用例为 0。

#### MySQL专属函数隔离

以下四组为**待新增、未运行**的验收目标，各表达式独立 `SELECT <表达式> AS v`。MySQL 指 AsterSQL MySQL 入口及相应参考端；TIDB_VERSION 参考使用记录完整版本的 TiDB/本仓库 Go，普通 MySQL 无此专属函数不作成功参考。固定无同名用户/扩展函数、UTF-8、记录 MySQL 会话环境与构建信息；PG 用内建查找环境，不能用语法错误替代名称拒绝。

| 场景 | MySQL 成功、NULL、类型与错误保持目标 | PG 可见性拒绝目标 |
| --- | --- | --- |
| FUNC-MY-IFNULL | `IFNULL(CAST(NULL AS CHAR),CAST('fallback' AS CHAR))`→非 NULL 'fallback'/自身字符串；两参均 CAST(NULL AS CHAR)→SQL NULL/同类字符串。`IFNULL(1,2)`→1/自身整数；一参数 `IFNULL(1)` 保留参数数目错误，无成功行。另以可控报错表达式作备用分支验证首参非空不求值，局部 Error 断言不替代 SQL 证据。精确 flags/宽度/错误字段待采。 | `ifnull(NULL::text,'fallback'::text)` 及全 typed NULL 版本→42883，无成功行；不得改写 COALESCE。 |
| FUNC-MY-DATE | `DATE_FORMAT(CAST('2024-01-02 17:03:04' AS DATETIME),'%Y-%m-%d %H:%i:%s')`→非 NULL '2024-01-02 17:03:04'/自身字符串；日期或模板单独 typed NULL→SQL NULL/字符串；同有效日期空模板→非 NULL 空串。一参数 DATE_FORMAT('2024-01-02') 保留参数错误；零日期警告/错误按固定 sql_mode 对照 Go，精确诊断待采，不能因 PG 兼容改写。 | `date_format(TIMESTAMP '2024-01-02 17:03:04','%Y-%m-%d %H:%i:%s')` 及日期/模板 typed NULL→42883；不翻译为 to_char。 |
| FUNC-MY-JSON | `JSON_EXTRACT('{"a":1,"b":2}','$.a','$.b')`→非 NULL JSON [1,2]；`JSON_EXTRACT('{"a":null}','$.a')`→JSON null（IS NULL 为 false）；同文档 '$.missing'、SQL NULL 文档或路径→SQL NULL，结果类型仍 JSON/code 245。`JSON_EXTRACT('{"a":1}','$InvalidPath')`→路径错误、无成功行；非法 JSON 文档另采自身转换错误，不吞为缺失/NULL，精确诊断待采。 | `json_extract('{"a":1}'::json,'$.a'::text)`、文档/路径 typed NULL 和非法路径版本均→名称无匹配 42883；不能先运行 MySQL 路径解析或返回 PG JSON 值。 |
| FUNC-MY-ADMIN | `TIDB_VERSION()`→非 NULL 构建信息/自身字符串，内容等于同一服务构建的 GetTiDBInfo，不能固定参考 TiDB 品牌串冒充本构建；零参不适用 NULL 输入。`TIDB_VERSION(NULL)`→参数数目错误，无成功行；结果类型/flen 不由第一行推断，精确列定义/错误字段待采。 | `tidb_version()` 及 `tidb_version(NULL::text)`→42883，无成功行；不重写 PG version()，不泄露 MySQL 工厂。 |

四组分别重复 PG 未限定名与 pg_catalog 限定名；PG Q 和扩展 Parse 均须在分析阶段拒绝且无成功 RowDescription/DataRow，记录错误实际字段/阶段，不能拿缺工厂错误充作 42883。MySQL 对成功/NULL 采 Query、Prepare/Execute、参数值/NULL 重绑、零行列定义，逐列核对 code/flags/charset/length/decimals、载荷与警告。两个 PG 连接与一个 MySQL 连接交错重复调用，验证名称/工厂缓存不串方言；MySQL 原错误与 Go/TiDB 参考逐条核对。PG 未限定名仅在夹具排除同名用户函数时要求拒绝，不扩大为禁止用户定义函数。全部四组有效运行用例数为 0，当前仅完成契约证据与场景覆盖。

#### 运算符调用契约

以下四组为待新增、未运行目标，各表达式独立 `SELECT <表达式> AS v`；PG 使用显式类型，MySQL 使用自身 CAST，禁止给 MySQL 输入 `::` 后将语法拒绝当语义对照。两入口各自与对应参考端核对，不要求同符号得到同值。无同名用户/扩展遮蔽，固定 UTF-8，记录 MySQL sql_mode（除零组固定普通 SELECT、非严格模式及 ERROR_FOR_DIVISION_BY_ZERO 开启）、div_precision_increment=4 和字符环境。

| 场景 | PG 入口目标：输入、值/NULL、类型、错误 | MySQL 入口目标：可见性及自身行为 |
| --- | --- | --- |
| OP-CALL-DIV：整数/非整数 | `5::int4 / 2::int4`→2，`(-5)::int4 / 2::int4`→-2，均非 NULL/int4；`5::numeric / 2::numeric`→精确 2.5/numeric，不固定文本尾零；`NULL::int4 / 2::int4`→SQL NULL/int4。补一元 `-(5::int4)`→-5/int4、`-('-2147483648'::int4)`→22003，区分一元/二元。`('-2147483648'::int4) / (-1)::int4`→22003，无成功值。 | `CAST(5 AS SIGNED) / CAST(2 AS SIGNED)`→2.5000/自身 DECIMAL，负数版本→-2.5000；`CAST(5 AS SIGNED) DIV CAST(2 AS SIGNED)`→2/BIGINT，负数版本→-2；`CAST(5 AS DECIMAL(2,0)) / CAST(2 AS DECIMAL(2,0))`→2.5000/DECIMAL。typed NULL 除以 2→SQL NULL/相应类型；一元 `-CAST(5 AS SIGNED)`→-5/自身 signed 整数。以上均可见，成功无错误，不把 PG int4 结果宽度写入 MySQL。 |
| OP-CALL-ZERO：除零 | `1::int4 / 0::int4` 与 `1::numeric / 0::numeric`→22012，无成功值/NULL；`NULL::int4 / 0::int4`→SQL NULL/int4。常量折叠可提前失败；参数版 `$1::int4 / $2::int4` 用 1/0 验证执行错误，用 NULL/0 验证同型 NULL。 | 普通 SELECT 的 `CAST(1 AS SIGNED)/CAST(0 AS SIGNED)` 和 `CAST(1 AS SIGNED) DIV CAST(0 AS SIGNED)`→SQL NULL，分别保持 DECIMAL/BIGINT，收集除零警告；NULL 左参→NULL。警告精确码/SQLSTATE/消息及正式链传播仍待 Go/参考端采集，不拿局部 error_ctx 的错误证明 SQL 模式已接通；DML 严格模式不在本组。 |
| OP-CALL-CONCAT：文本拼接与 NULL | `'a'::text \|\| 'b'::text`→非 NULL 'ab'/text；`'a'::text \|\| NULL::text` 和 `NULL::text \|\| 'b'::text`→SQL NULL/text；`''::text \|\| 'b'::text`→'b'。合法 text/text 无错误，不能用 concat 函数的 NULL 行为代替 `\|\|`。 | PIPES_AS_CONCAT 关闭时 `1 \|\| 0`→1、`NULL \|\| 0`→SQL NULL/自身整数；开启时 `CAST('a' AS CHAR) \|\| CAST('b' AS CHAR)`→'ab'/自身字符串，单侧 CAST(NULL AS CHAR)→SQL NULL/字符串。结果/警告按模式分别核对，PG 不继承 MySQL 开关；不能把关闭模式下的逻辑 OR 误作文本拼接支持。 |
| OP-CALL-NULLSAFE：NULL 安全比较 | int4 对分别 `NULL IS NOT DISTINCT FROM NULL`→true，`NULL IS NOT DISTINCT FROM 1`→false，`1 IS NOT DISTINCT FROM 1`→true，`1 IS NOT DISTINCT FROM 2`→false（两侧均加 ::int4）；对应 IS DISTINCT FROM 逐项取反，均非 NULL/bool。对照 `NULL::int4 = 1::int4`→SQL NULL/bool；合法输入无错误。 | 使用同值的 CAST(... AS SIGNED) 与 `<=>`，两空/一空/等值/异值→1/0/1/0，均非 NULL/自身布尔整数表达式；普通 `=` 一空→SQL NULL。PG 不批准 `<=>` 内建别名，`1::int4 <=> 1::int4` 在无用户定义夹具下目标 42883，无成功行；不能把 nulleq 名称注册作为 PG 谓词接线证明。 |

PG `/` 和文本 `||` 同时核对未限定写法与 `OPERATOR(pg_catalog./)` / `OPERATOR(pg_catalog.||)`，NULL 安全谓词无 OPERATOR 同名形式，不适用该检查。四组成功/NULL 在 Q 与扩展 Parse/Bind/Describe/Execute 核对值、OID/typlen/typmod、文本/二进制格式，参数重绑与零行描述不改型；MySQL 核对 Query、Prepare/Execute、参数 NULL 重绑及零行列定义。错误可能在分析、折叠或执行阶段，分别保留原字段，不能强制所有错误都在 Parse；PG 参数版补执行与恢复证据。交错 PG/MySQL 连接与 MySQL 两种 pipes 模式，确认缓存保留方言/会话签名。元数据及诊断尚未采集，四组有效运行用例数均为 0，文档结构通过不能升级为功能通过。

#### 普通聚合调用契约

以下 AGG-CALL 五组全部为**待新增、未运行**目标。每组分别比较 AsterSQL PG/参考 PG 与 AsterSQL MySQL/自身参考端，名称均目标可见；PG 另重复 pg_catalog 限定名。固定无同名用户/扩展遮蔽、UTF-8、MySQL div_precision_increment=4，合法项无错误，除显式 SQL NULL 外非 NULL。PG t 用表列出的定型 VALUES；MySQL 用等值 `SELECT CAST(... AS SIGNED/DECIMAL/DOUBLE) ... UNION ALL ...` 派生表，不发送 PG 的 ::/VALUES/FILTER 语法来冒充比较。

| 场景 | PG 输入、值/NULL/类型及错误目标 | MySQL 自身入口目标与限制 |
| --- | --- | --- |
| AGG-CALL-EMPTY：无行全局 | `SELECT count(*) AS n,count(x) AS c,sum(x) AS s,avg(x) AS a FROM (SELECT 1::int4 AS x WHERE false) t`→一行 (0,0,NULL,NULL)，类型 (int8,int8,int8,numeric)。补 `GROUP BY x`→零行，四列类型仍固定。 | 等值空表 `(SELECT CAST(1 AS SIGNED) AS x WHERE 1=0) t` 运行同投影→一行 (0,0,NULL,NULL)，BIGINT/BIGINT/DECIMAL/DECIMAL；加 GROUP BY x→零行，保留列定义。 |
| AGG-CALL-NULL：全 NULL | 对 `(VALUES(NULL::int4),(NULL::int4)) t(x)` 运行同四列投影→(2,0,NULL,NULL)，类型同 EMPTY；区别无行与两行全 NULL。 | `(SELECT CAST(NULL AS SIGNED) AS x UNION ALL SELECT CAST(NULL AS SIGNED)) t`→同值/NULL，自身 BIGINT/BIGINT/DECIMAL/DECIMAL；不把 0 行、NULL 和 decimal 零混同。 |
| AGG-CALL-GROUP：分组与状态隔离 | 对 `(VALUES(1,1::int4),(1,3::int4),(2,NULL::int4)) t(g,x)`，`SELECT g,count(*),count(x),sum(x),avg(x) ... GROUP BY g ORDER BY g`→(1,2,2,4,2)、(2,1,0,NULL,NULL)，g 为 int4，其余同 EMPTY。每组加 `FILTER (WHERE false)`→各自 (g,0,0,NULL,NULL)，两组仍存在；NULL 分组键另补两行 NULL 键应同组。 | 等值 SIGNED 派生表，同无 FILTER 查询→相同值/NULL，g 为自身整数，其余自身类型。保留 NULL 键同组目标；FILTER 版本不要求 MySQL 可见，不把删除所有行的 WHERE 改写作等价证据。 |
| AGG-CALL-DISTINCT-FILTER：重复/筛选/排序 | t=`(VALUES(1::int4,true),(1::int4,false),(2::int4,true),(NULL::int4,true),(9::int4,NULL::bool)) t(x,keep)`；普通四列投影→(5,4,13,3.25)。另独立查 `count(*) FILTER (WHERE keep),count(DISTINCT x) FILTER (WHERE keep),sum(DISTINCT x ORDER BY x DESC) FILTER (WHERE keep),avg(DISTINCT x) FILTER (WHERE keep)`→(3,2,3,1.5)，类型同 EMPTY。`sum(DISTINCT x ORDER BY keep)` 须拒绝，无成功行，精确诊断待采；普通数值结果相同不能单独证明排序器执行过，需局部有序输入轨迹证据。 | 等值 SIGNED x 与 1/0/NULL keep，普通四列→(5,4,13,3.25)/自身类型；不加 FILTER 的 `COUNT(DISTINCT x),SUM(DISTINCT x),AVG(DISTINCT x)`→(3,12,4)。用 `COUNT(CASE WHEN keep THEN 1 END),COUNT(DISTINCT CASE WHEN keep THEN x END),SUM(DISTINCT CASE WHEN keep THEN x END),AVG(DISTINCT CASE WHEN keep THEN x END)`→(3,2,3,1.5)，仅本夹具值对照，不批准通用 FILTER 改写或 MySQL 聚合内 ORDER BY。 |
| AGG-CALL-NUMERIC：结果身份与错误 | 各 T∈int2/int4/int8/numeric/float4/float8，分别 `SELECT sum(x) AS s,avg(x) AS a FROM (VALUES(1::T),(2::T)) t(x)`→(3,1.5)；SUM 类型依次 int8/int8/numeric/numeric/float4/float8，AVG 前四 numeric、后两 float8。追加 `sum('x'::text)`→42883；sum(x) 对两个合法 '9223372036854775807'::int8→18446744073709551614/numeric，不得先按 i64 溢出。 | 分别 SIGNED、DECIMAL(10,2)、DOUBLE 的 1/2 派生表，SUM/AVG→3/1.5，前两自身 DECIMAL、后一 DOUBLE，精确文本尾零与 metadata 待采；SIGNED 的两个 9223372036854775807 求和保持自身 DECIMAL 域。PG sum(text) 拒绝不强加 MySQL，字符串转换/警告依自身参考端另采。 |

每组 PG 成功/NULL/零行覆盖 Q 与 Parse/statement Describe/Bind/portal Describe/Execute；参数化输入用 `$1::int4` 等固定类型，text/binary 参数×结果四组合，非空/NULL 重绑不换 OID/typmod，Execute 不重发描述。count=0 与 SQL NULL 分开核对载荷，MySQL Query/Prepare/Execute 同样记录列定义和 NULL bitmap；错误独立记录真实阶段/字段，不能用缺工厂或 codec 失败当目标拒绝。两 PG 连接与 MySQL 交错调用，验证签名、状态及会话策略不串线。

局部/最终与下推另需计划/状态证据：将 1、1、2 分在不同局部分区（夹带空分区），核对全局 DISTINCT count=2、sum=3、avg=1.5；将 [1] 与 [2,3,4] 分开，AVG 最终=2.5，不能平均 1 与 3 得 2。比较单阶段及合法合并的值/NULL/类型/错误，验证 FILTER 各自独立及排序信息保留，浮点另外记录次序/舍入差异；未获此证据不批准共享或下推。当前五组及此门禁有效运行用例均为 0，结构检查不表示功能通过。

#### 窗口函数调用契约

以下 WIN-CALL 五组为**待新增、未运行**目标。每项分别比较 AsterSQL PG/参考 PG、AsterSQL MySQL/自身参考端；五种名称均目标可见，PG 再核限定名。PG 使用下面定型 VALUES；MySQL 以等值 CAST(... AS SIGNED) 的 SELECT/UNION ALL 派生表替换，投影/窗口使用自身合法语法。固定 UTF-8、无用户函数遮蔽；外层 ORDER BY id（分组例按 p,id）确定展示，合法项无错误。类型/NULL/wire 沿本块模板。

| 场景 | PG 输入与预期 | MySQL 自身入口及证据限制 |
| --- | --- | --- |
| WIN-CALL-PEER：同序/分区 | t=`(VALUES(1,1,10::int4),(1,2,10::int4),(1,3,20::int4),(2,4,10::int4)) t(p,id,x)`；投影 p,id、`row_number() OVER(PARTITION BY p ORDER BY x,id)` AS n、`rank() OVER(PARTITION BY p ORDER BY x)` AS r→(1,1,1,1),(1,2,2,1),(1,3,3,3),(2,4,1,1)，n/r 为 int8。另 row_number 只 ORDER BY x 时前两行编号集合为 {1,2}，不绑定 id。 | 等值 SIGNED 派生表与同窗口目标相同值、分区重置，排名为自身整数元数据待采；不从该值相同批准跨方言类型共享。 |
| WIN-CALL-FRAME：默认/显式 | t=`(VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)`；`last_value(x) OVER(ORDER BY x)`→[10,10,20]；显式相同 RANGE→同值；`OVER(ORDER BY x,id ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING)`→[20,20,20]；`OVER()`→每行取无序分区末值，不能固定为 20。另 `OVER(ORDER BY x GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING)`→[20,20,20]，`RANGE BETWEEN CURRENT ROW AND 5 FOLLOWING`→[10,10,20]，`ROWS BETWEEN CURRENT ROW AND CURRENT ROW`→当前 x。last_value 均 int4。 | 默认和显式 ROWS/RANGE 同输入按自身参考验证值目标；GROUPS 不要求 MySQL 可见，记录其自身拒绝，不能强行改写。AsterSQL PG 若 GROUPS/EXCLUDE 等未实现须返回明确能力错误，不得标该成功目标通过；另同 t 的 last_value(x) OVER(ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE CURRENT ROW)→[NULL,10,10]/int4；采 PG 合法结果/本地能力拒绝证据，精确诊断待采。 |
| WIN-CALL-EMPTY-SINGLE：空/单行 | 对 `(SELECT 7::int4 AS x WHERE false) t` 投影 `row_number() OVER(),rank() OVER(),lag(x) OVER(),lead(x) OVER(),last_value(x) OVER()`→零行，类型(int8,int8,int4,int4,int4)。改为单行 7→(1,1,NULL,NULL,7)；单行 last_value(x) 的 `ROWS BETWEEN 1 FOLLOWING AND 1 FOLLOWING`→NULL/int4，行仍存在。 | 等值 SIGNED 空/单行表，同五投影值/NULL/行数目标；空帧 last_value→NULL/自身值类型，列定义在零行仍存在。当前测试仅空帧聚合和 row_number 意图，不是 last_value 证据。 |
| WIN-CALL-OFFSET：分区越界/动态 | t=`(VALUES(1,10::int4,1),(2,20::int4,2),(3,30::int4,0)) t(id,x,o)`，均 OVER(ORDER BY id)：lag(x)→[NULL,10,20]，lead(x)→[20,30,NULL]；lag(x,9,-1) 与 lead(x,9,-1)→[-1,-1,-1]；lag(x,0)→[10,20,30]；lag(x,o,id::int4)→[1,2,30]；lead(x,NULL::int4,-1)→全 NULL；lag(x,-1)→[20,30,NULL]。均 int4。另给 lag/lead 显式当前行 ROWS 帧，结果仍按整分区位置取值。 | 常量 0/1/9 与默认值夹具按自身合法调用验证相同值目标；动态、负及 NULL offset 不套 PG，单独采自身接受/拒绝和错误字段，当前未采。Rust 无符号常量限制不能当 PG 目标。 |
| WIN-CALL-NULL：值 NULL/排序 NULL | t=`(VALUES(1,10::int4),(2,NULL::int4),(3,30::int4)) t(id,x)`，OVER(ORDER BY id)：lag(x,1,-1)→[-1,10,NULL]，lead(x,1,-1)→[NULL,30,-1]，last_value(x)→[10,NULL,30]；last_value(x) 配全分区 ROWS→[30,30,30]。再按 x ASC NULLS LAST 求 rank→id 次序 [1,3,2]，DESC NULLS FIRST→[3,1,2]。IGNORE NULLS 调用拒绝，无成功值，确切诊断待采。 | 值 NULL 的同窗口目标同值、不得用 default 替换存在的 NULL；默认排序不同，使用自身合法 `ORDER BY (x IS NULL),x` 对照 PG ASC NULLS LAST 的排名值，不能据改写证明 PG NULLS 语法已支持；自身默认排序/错误另采。 |

每组成功/NULL/零行均须覆盖 PG Q 和 Parse/statement Describe/Bind/portal Describe/Execute；参数化 x/offset 用固定 `$1::int4`，text/binary 参数×结果四组合，重绑 NULL 不换描述，Execute 不重发 RowDescription；MySQL Query/Prepare/Execute 记录自身列定义/NULL bitmap。两 PG 连接与 MySQL 交错运行，核对分区状态及方言隔离；缓冲/流水线、跨 Chunk 的同夹具须值/NULL/类型一致。拒绝例独立执行 `SELECT row_number()`，以及单行 7 上 last_value(x) 的 `ROWS BETWEEN CURRENT ROW AND 1 PRECEDING`、`ROWS BETWEEN -1 PRECEDING AND CURRENT ROW`、`ROWS BETWEEN NULL::int4 PRECEDING AND CURRENT ROW`，均无成功值；分别记录缺 OVER、非法帧边界/offset、未实现帧形式的实际阶段和错误，不把缺工厂/codec 失败当合规拒绝。五组当前运行数均 0，描述符与测试源码不代替 SQL/协议证据。

#### 有序集合聚合契约

下列四组是目标用例清单。统一 `WITH t(x) AS (VALUES (1::int4),(2::int4),(3::int4),(4::int4))`，分别执行 `SELECT percentile_disc(p::float8) WITHIN GROUP (ORDER BY x ASC) AS v FROM t` 与 `SELECT percentile_cont(p::float8) WITHIN GROUP (ORDER BY x::float8 ASC) AS v FROM t`；p 为用例占位符，执行时替换为表中比例字面量或已定型 $1，非未绑定列名。每项独立执行以免错误掩盖其他结果，固定 disc int4、cont float8，不借 unknown/cast 歧义改变被测签名。

| 场景 ID/输入 | PG 目标值、NULL、类型或错误 | 两入口可见性与证据等级 |
| --- | --- | --- |
| ORDERED-MEDIAN：上述四值，p=0.5；再将 ORDER BY 改 DESC | ASC disc=2、cont=2.5；DESC disc=3、cont=2.5，各一行、非 NULL、类型分别 int4/float8。另用 ASC 和 FILTER (WHERE x<=2) 得 disc=1、cont=1.5，验证只筛选本聚合输入。 | PG 两条协议路径均应匹配；MySQL 入口拒绝该 PG 调用，原 APPROX_PERCENTILE 不受影响。官方规则推导目标＋本地静态证据，非 SQL 实测。 |
| ORDERED-EMPTY：FROM t WHERE false；以及 t 改为两个 NULL::int4，p=0.5 | 两者均一行 SQL NULL，int4/float8 描述不变；附 `GROUP BY x` 的空输入应零行且仍可 Describe，不能造一个 NULL 组。 | PG 与 MySQL 隔离同上；NULL 载荷/零行元数据待双路径采集，没有运行通过证据。 |
| ORDERED-NULL：t 为 NULL::int4、1、2、3、4、NULL；另测 p=NULL::float8 | p=0.5 的结果仍 disc=2、cont=2.5；NULLS FIRST/LAST 不改变该结果。typed NULL 比例两者为同型 NULL，不将排序输入 NULL 计入分母。 | PG 应成功，MySQL 不获得 PG 可见性；依据官方 NULL 规则，状态及 codec 未运行。 |
| ORDERED-FRACTION：四值输入分别 p=0、1、-0.01、1.01 | ASC p=0 两者取 1，p=1 两者取 4，各保留 int4/float8；负值和大于 1 均错误、无成功值，精确错误字段/时机待参考采集。另列 NaN/Infinity、非法调用形态错误采集，不预填未核实的结果。 | PG 扩展协议以已定型 float8 参数 Bind 重测边界，防缓存省略校验；MySQL 不调用 PG 实现。只有目标与静态证据，错误及协议未实测。 |

人工验收须同时核对 direct/排序输入载体、每组状态生命周期与上述输入/结果/类型/错误/可见性映射。PG Q 与扩展协议分别采值、NULL、OID/长度/typmod、格式与 ErrorResponse；MySQL 入口采自身错误码/SQLSTATE/消息并保留旧函数回归。当前文档结构/路径/差异检查通过只能证明文档交付，不提升 S/V/C/M 或 D/H/P，不表示有序集合聚合已经接线。

#### 集合返回函数契约

下列五组是目标用例，不是已运行 SQL。除关联组外统一运行 `SELECT g.v FROM generate_series(a::int4,b::int4,h::int4) AS g(v)` 与 `SELECT generate_series(a::int4,b::int4,h::int4) AS v`；a/b/h 为执行前替换的表中参数占位符，unnest 组使用表列 SQL。结果序列表示生成次序；客户端比较若要求结果有序，应增加显式排序（需保留原展开顺序时另验 WITH ORDINALITY），不能依赖无 ORDER BY 的最终输出顺序。

| 场景 ID/输入 | PG 目标值、NULL、列类型/错误 | 两入口可见性与证据等级 |
| --- | --- | --- |
| SRF-DIRECTION：a/b/h=(1,3,1)、(3,1,1)、(3,1,-1)、(1,3,-1)、(2,2,1) | 分别生成 1/2/3、零行、3/2/1、零行、单行 2；单列 int4，零行也保持描述，不能生成一行 NULL。 | PG FROM/投影及 Q/扩展协议均为目标；MySQL 拒绝此 PG SRF，保留旧函数。官方规则推导＋本地静态证据，未运行。 |
| SRF-ZERO：a/b/h=(1,3,0) | 错误、无成功值，不循环、不改成默认步长；错误字段/时机待参考采集，不能以本地 panic 代替 ErrorResponse。 | PG 两位置、两协议采错误；MySQL 按自身协议采拒绝，不复用 PG 错误文本；未实测。 |
| SRF-NULL：分别只把 a、b、h 换 NULL::int4，其余为 1/3/1 | 各零行、单列 int4 描述不变；typed NULL 不变成一个 NULL 结果值。扩展协议以已定型参数 Bind NULL 重测。 | PG 可见、MySQL 隔离同上；分析后身份、NULL 基数及 codec 未运行。 |
| SRF-ARRAY：`SELECT u.v FROM unnest(ARRAY[2,NULL,2]::int4[]) AS u(v)`；另将数组换 ARRAY[]::int4[]、NULL::int4[]；投影对应 `SELECT unnest(…) AS v` | 三行 2/SQL NULL/2、零行、零行，均单列 int4；不得混淆 NULL 数组、空数组和 NULL 元素。重复值不去重。 | PG 两位置/两协议验证，MySQL 不获得 PG 数组 SRF；类型依赖第 3 章，本轮仅官方推导及静态证据。 |
| SRF-CORRELATED：`WITH t(id,n) AS (VALUES (1::int4,2::int4),(2,0),(3,NULL::int4)) SELECT t.id,g.v FROM t CROSS JOIN LATERAL generate_series(1::int4,t.n,1::int4) AS g(v) ORDER BY t.id,g.v` | 两行 (1,1)/(1,2)，两列 int4；改 LEFT JOIN LATERAL 并加 ON true，应再有 (2,NULL)/(3,NULL)。改为投影 `SELECT t.id,generate_series(1::int4,t.n,1::int4) AS v FROM t` 应仅两行；验证每外行重置，空/NULL 不泄漏前一次状态。 | PG Q 与已定型扩展协议采集；MySQL 拒绝此 PG 调用形态。LEFT 的补 NULL 来自外连接，不改变函数自身零行语义；未实测。 |

人工审阅五组的输入、基数/值/NULL、固定类型、错误、两入口与证据层级映射。另以 `SELECT generate_series(1,2) AS a,generate_series(10,12) AS b` 定义多投影边界目标 (1,10)/(2,11)/(NULL,12)，禁止笛卡尔乘积或最短截断；CASE/COALESCE 内调用为分析错误采集边界。首批实现若未覆盖这些扩展形态须显式能力拒绝，不宣称完整投影支持。生命周期验收另采早停、取消、重扫与分段 Execute；当前文档检查不构成这些运行行为通过。

#### 章节验收与后续交接

以下为全章执行交接矩阵，均是**待执行行为目标**。具体 SQL、预期及场景 ID 复用前 16 块验收表，不复制易漂移的另一套 SQL；对每个可见成功签名逐格采证，对隐藏/非法调用单列拒绝记录。PG 与 MySQL 分别对自身参考端，不要求两种方言互相同型。

| 验收维度 | PG 入口 | MySQL 入口 | 代表落点与当前状态 |
| --- | --- | --- | --- |
| 值与基数 | 值精确域、浮点舍入/格式；空全局聚合一行与 SRF 零行分开 | 自身数值/文本/JSON、行数及警告按模式核对 | FUNC-ID/SPLIT、AGG-CALL-EMPTY、SRF-DIRECTION；仅目标/静态取证 |
| NULL | typed NULL、全 NULL、首行 NULL、空/NULL 数组及元素 NULL 不混同 | 自身 NULL bitmap/载荷，JSON null 不当 SQL NULL | FUNC-ID-ABS-TYPED-NULL、FUNC-ARR、FUNC-MY-JSON、SRF-ARRAY；未运行 |
| 类型与转换 | 解析固定参数/返回身份、宽度/精度及 I 资格，Bind 换值不选签名 | 自身 SIGNED/DECIMAL/字符串/JSON 类型及转换规则 | FUNC-ID/RES、AGG-CALL-NUMERIC；精确 MySQL 修饰符仍缺证 |
| wire 元数据 | Q、statement Describe、portal Describe；OID/typlen/typmod/列名/来源与格式，text/binary×适用参数格式，NULL/零行不改型 | Query、Prepare/Execute 自身 code/flags/charset/length/decimals、参数重绑及零行列定义 | 首块公共协议规则及各块协议段；当前编码源码/独立测试意图不是函数全链通过 |
| 错误/警告 | 逐场景 SQLSTATE 与 S/V/C/M、适用 D/H/P，记录分析/折叠/执行阶段及恢复；缺工厂/codec 不冒充语义拒绝 | 原错误码/SQLSTATE/消息/警告及 sql_mode，不直接套 PG 诊断 | FUNC-ID-ABS-ERROR、FUNC-REG-UNLINKED、FUNC-RES-NO-MATCH/AMBIG、OP-CALL-ZERO、ORDERED-FRACTION、SRF-ZERO；未采字段明确缺口 |
| 可见性与隔离 | 无同名遮蔽夹具下核对内建/限定名；两个 PG 会话交错名称、签名、状态与缓存 | 同时活动 MySQL 连接保留旧名称/工厂/模式；隐藏 PG 专属调用用合法 MySQL 语法验证 | FUNC-REG、FUNC-MY、FUNC-SYS-ISOLATION；不以全局注册切换模拟方言，未运行 |
| 状态、优化与下推 | 聚合合并、窗口帧、ordered-set direct/排序、SRF 重扫/耗尽/暂停恢复；与本地执行逐签名对照 | 原执行路径和自身状态/优化契约保持，独立采证 | 六模型各自验收段＋本块门禁；新增共享/别名/下推批准均 0 |

人工文档验收结论：前 16 块在五节中可定位，来源步骤 1–4、四类代表、全模板字段和六模型均有正文/验收落点；与第 1 章证据分级、第 2 章所属连接方言、第 3 章固定类型/E/A/I/Describe 规则一致，未修改前序正文。未来依赖缺口被明确交接，不作为本轮文档阻塞，也不视作已实现。结构/引用/差异检查及此人工覆盖审阅仅证明章节交付；correctness/兼容性为目标、运行有效用例 0、性能未验证。Ready 文档范围使用任务规定的可执行检查；仓库 `.agents/skills/tidb-verify-profile/SKILL.md` 缺失，未声称运行该技能或代码检查。

### 后续细分

#### 三层身份与契约模板

下一块先承接本模板，细化注册/重载的方言可见性、名称环境、候选排序、隐式 cast 与错误；依赖第 2 章身份/名称及第 3 章类型/转换/元数据，不递归修复缺失子系统。随后各函数族与独立调用模型只补自身实例和窄边界，引用本模板及对应官方来源；缺证项补证后再批准共享别名或下推。本块只给文档交接，不生成代码任务。

来源映射：蓝图任务 4 步骤 1 的三层契约由身份表与标量候选贯通，步骤 3 的所有字段由统一模板覆盖；编号任务的同名不同签名、同值不同类型、typed NULL 分别由 FUNC-ID-ABS-SIGNATURE/SAME-VALUE/TYPED-NULL 覆盖，ERROR 补充域/签名拒绝。与前序章节的连接方言、E/A/I 资格和固定元数据规则一致，无需修改第 1–3 章。其余来源步骤和完整章节验收由后续文档任务承接，不随本块完成升级。

#### 注册工厂与方言可见性

下一块只细化已过滤候选中的重载、隐式 cast 与歧义，不重建名称解析器；函数族块再补 date_trunc/IFNULL 的规范实现 ID、NULL/优化属性与 wire 细节。后续实现先为现有正式构造链补最小方言接线及聚焦门禁证据，缺工厂问题单独追踪，不递归补齐函数库；系统目录事实仍交工作流 6，协议诊断交 7。

来源映射：蓝图任务步骤 1 的方言选择层落在四阶段证据表；步骤 2 的 PG/MySQL 专属代表落在 FUNC-REG-PG-ONLY/MYSQL-ONLY；步骤 3 的可见性、别名及模板字段落在差异表与四族验收，UNKNOWN/UNLINKED 区分名称缺失和实现缺口。首块三层身份、NULL/元数据与第 2 章连接身份、第 3 章转换资格保持一致；本块未修改前序正文或升级任何运行状态。

#### 重载与隐式转换选择

后续实现只交接三个窄边界：接收第 2 章可见集合及第 3 章类型/I 资格的候选选择；已选签名到正式工厂及合法 coercion 的局部接线；参数/结果身份在 Prepare/Describe/Execute 的连续证据。缺工厂、完整类型系统、UDF/多态、目录和协议设施分别追踪，不借本块补齐。可变/default 的完整开放须先补载体与聚焦解析证据，本轮不生成实现任务。

来源映射：蓝图步骤 1 的方言签名层与步骤 3 的重载/cast/NULL/返回和 wire 字段，由本块目标段、模板差异表及 FUNC-RES 五组覆盖；精确签名、unknown、NULL、无匹配、歧义逐行有输入与类型/错误目标，其他模板字段明确不适用或缺口。前两批已交付的名称门禁、首块 strict/类型身份与第 3 章 E/A/I、unknown 参数及固定描述保持一致。结构、引用和差异审阅仅证明文档交付；正确性/兼容性目标未实测，没有性能结论，不升级任何功能支持状态。

#### 等价共享与别名门槛

后续只交接 COALESCE 共同类型到惰性正式调用、NULLIF 比较签名到返回身份、text 输出 collation 和两入口证据四个窄边界，依赖第 3 章及前块名称/转换资格。先补代表签名证据再讨论别名或下推；缺工厂、类型系统与协议设施分别追踪，不在本块补建，不提前生成代码任务。

来源映射：蓝图步骤 2 第一类由两候选和逐签名批准门槛覆盖；步骤 3 的名称/可见性、重载/cast、NULL、collation/时区、确定性、返回、wire、下推和错误均映射模板差异表及 FUNC-EQUIV 五组。普通值、全 NULL、混型、未达错误、collation 分别定位 VALUE/ALL-NULL/MIXED/UNREACHED-ERROR/COLLATION；每组含两入口输入与目标或明确缺口。人工审阅已确认与前块“不默认批准别名”、第 3 章定型和固定描述一致。本轮仅文档完成，正确性/兼容性仍为未实测目标，性能无结论。

#### 同名异义分流

后续仅交接 concat 的方言 NULL/输出策略到正式构造链、极值特殊表达式的共同类型/NULL/参数边界、已定型比较/拼接可复用域及两入口描述证据；依赖前块名称隔离、第 3 章类型/I 资格和 server 元数据，不扩展为函数库或执行器重建，不生成代码任务。

来源映射：蓝图步骤 2 第二类由 concat/greatest/least 分流理由和独立语义 ID 覆盖，步骤 3 全字段由本块模板差异表覆盖；NULL、混型、字符串排序、参数个数分别映射 FUNC-SPLIT-NULL/MIXED/COLLATION/ARITY，两入口各有输入、值/NULL/类型或拒绝目标及缺口。与前块“不因同名/局部实现批准共享”、PG 特殊表达式边界和第 3 章固定身份一致，前序正文未修改。文档正确性/兼容性目标尚未实测，性能无结论。

#### 时间截断与提取

后续仅交接已定型三种源到截断/字段内核、numeric/float8 返回策略到正式工厂、会话/显式区域上下文到日历计算、固定结果身份到 server 的四个窄边界；依赖第 2 章语法、前块注册/重载、第 3 章类型/codec 和工作流 5 的会话区域。先补本块五组及两入口证据，再考虑共享算法与下推；不生成代码任务，不递归补齐时间库或协议设施。

来源映射：蓝图步骤 2 的 PG 核心 date_trunc/date_part 由本块重载表及 FUNC-TEMP 覆盖，步骤 3 全部模板字段由本块差异表承接；跨日、区域、DST、interval、NULL/非法单位分别定位 DAY/ZONE/DST/INTERVAL/NULL-INVALID。date_part 与 EXTRACT 返回类型分别有 §9.9.1 和 pg_proc.dat 证据，精度/两入口可见性/错误及 wire 缺口均明确。人工审阅与第 2 章 EXTRACT 语法、第 3 章三种身份和前块“不凭同名批准别名”一致，未重写前序正文。correctness/兼容性仅为文档目标，运行行为未验证，性能无结论。

#### 格式化函数契约

后续仅交接已选源类型到时间/数值模板内核、locale/TimeZone 执行上下文、text 固定身份到正式工厂/server 三个窄边界；依赖前块 resolver、第 3 章类型/cast 和工作流 5 的会话生命周期。先补五族及双方言元数据证据，再讨论可复用组件；不递归补全 DATE_FORMAT、locale 或时间子系统，不生成代码任务。

来源映射：蓝图步骤 2 的 PG 核心 to_char 与 MySQL DATE_FORMAT 隔离由本块名称和签名表覆盖；步骤 3 全部模板字段由差异表承接。任务的时间格式、数值格式、字面量/修饰符、NULL、locale/时区分别定位 FUNC-FMT-TIME/NUM/TEMPLATE/NULL/CONTEXT，均含两入口输入、预期或明确缺口；返回文本元数据及错误阶段有公共验收规则。与前块不默认批准别名、第 3 章 text/时间/数值身份及第 2 章连接方言一致。correctness/兼容性仍是未实测目标，性能无结论；只完成文档契约，不修改第 1–3 章或其他契约块。

#### 数组函数契约

后续只交接第 3 章规范数组到维长/基数求值、共同元素类型到单元素追加、固定返回数组身份到正式工厂/server 三个窄边界；依赖前块名称/resolver 与第 3 章元素 I 资格、shape/lower/NULL/codec。先补五组与双入口证据，再讨论共享或下推；不生成代码任务，不借此补全数组或类型子系统。

来源映射：蓝图步骤 2 的 PG 核心数组代表由三签名及 FUNC-ARR 覆盖，步骤 3 的全部模板字段由本块差异表承接；空数组、数组 SQL NULL、NULL 元素、多维长度、非默认下界分别定位 EMPTY/NULL-ARRAY/NULL-ELEMENT/MULTIDIM/LOWER，含输入、值/NULL/类型/错误、两入口可见性和证据等级。元素提升及返回数组 OID 依赖落在 NULL-ELEMENT 与公共 wire 段。人工审阅与第 3 章基础数组两层 NULL/shape/lower、前块按分析固定身份和名称隔离一致，保留前序正文。correctness/兼容性仅为文档目标，运行未验证，性能无结论。

#### 范围函数契约

后续仅交接独立 range/subtype 身份及 empty/无界/开闭表示契约、已定型构造与规范化到正式标量链、固定 range/提取结果身份到 server codec 三个窄边界；第 3 章缺少通用 range 契约，须先补该前置文档与代表证据，不能由本块完成推断基础依赖已具备。先获得四组与两入口证据，再讨论共享/下推；本轮不生成代码任务。

来源映射：蓝图步骤 2 的 PG 核心 range 由 lower/upper/isempty/构造及 FUNC-RNG 四组承接，步骤 3 全部字段由统一模板差异表覆盖；空范围与 NULL、无界端点、开闭界、离散规范化分别定位 EMPTY-NULL/UNBOUNDED/BOUNDS/CANON，每组有输入、值/NULL/类型/错误与两入口可见性和证据等级。人工审阅与第 3 章未承诺 range、前块按分析固定身份及逐签名门禁一致，未改前序正文；correctness/兼容性仅为文档目标，运行未验证，性能无结论。

#### 必需系统函数契约

后续仅按首批客户端探测补数据库/schema 的正式标量接线，先向工作流 5/6/7 明确事实接口、读取时点与不可用诊断；再为版本、backend 身份和 pg_typeof 补具体客户端需求、窄 name/regtype 身份/codec 与独立验收。函数求值不拥有目录表或身份生命周期，不借本块扩大目录/权限/协议实现。范围、优先级与通用工厂接线仍须实施阶段重新核实，本轮不生成代码任务。

来源映射：来源步骤 2“PG 必需系统函数”由本块五候选和 FUNC-SYS 五组承接；步骤 4 的系统函数/目录责任由模块表、事实责任字段、范围外与上述接口交接承接。首块全部模板字段已逐项填写；权限过滤、类型内省、连接身份和两入口缺证均显式保留。文档完整性不表示 correctness/兼容性/性能已实测，未运行 AsterSQL/PG/MySQL 或 18.6 参考端。

#### MySQL专属函数隔离

后续仅交接现有候选过滤→四名称 MySQL 签名/工厂→原求值/返回元数据三个窄边界；依赖前块 resolver、任务 4 等价门槛、第 3 章类型和工作流 7 的双协议描述。实施前重新核实动态工厂及四组独立测试，缺工厂和精确列定义单独追踪；不递归补全时间/JSON/管理系统，本轮不生成代码任务。

来源映射：蓝图步骤 2 第四类由四个规范实现候选与 FUNC-MY-IFNULL/DATE/JSON/ADMIN 承接；步骤 3 的名称、可见性、重载/cast、NULL、collation/时区、确定性、返回/wire、下推、错误由本块统一模板各行覆盖。每类至少一组 MySQL 成功与 PG 拒绝、NULL 适用性/类型/错误保持均有明确目标或缺口；跨方言开放只引用任务 4 的证据门槛。correctness/兼容性为未实测目标，性能无结论；文档完成不表示函数支持通过。

#### 运算符调用契约

后续仅交接三个窄边界：整数/numeric `/` 的签名、值域与除零策略；text/text `||` 的 strict 拼接及模式隔离；int4 NULL 安全谓词的比较绑定与 bool 返回身份。各边界先核实 PG 语法节点 → 已选签名 → 正式工厂/求值链 → server 元数据，再决定能否复用局部算法，不另建一套求值器，不预先生成实现任务。

依赖第 2 章语法/名称环境、第 3 章类型及 cast、工作流 6 的运算符身份和 7/8 的协议/参考端证据；数组/range 运算符另族定义输入形状、重载与 NULL 后再验收。工厂或类型基础缺口登记为实现阶段依赖，不扩大本轮文档任务，也不阻塞已完成的文档交付；普通聚合沿下一块独立调用模型继续。

#### 普通聚合调用契约

后续仅交接三个窄边界：已选 aggregate 签名/固定返回身份到描述符；每聚合 FILTER、DISTINCT、排序到每组 transition/finalize；最终结果身份到 server 两协议描述。依赖第 2 章语法与名称、第 3 章类型/I 转换及前块 resolver，优先复用现有聚合链并先证明两条执行路径等价。局部/最终与下推须另过上述状态合并门禁；窗口、有序集合和 SRF 分别交后块，不提前生成代码任务。

来源映射：蓝图任务步骤 4 的 aggregate 独立调用模型由模块链及状态表覆盖；步骤 3 全部模板字段由本块差异表填写。无行全局、全 NULL、分组、重复/FILTER、数值返回分别映射 AGG-CALL-EMPTY/NULL/GROUP/DISTINCT-FILTER/NUMERIC，聚合内排序及局部/最终另有正反例和补证要求。人工审阅与首块三层身份、第 3 章固定类型及前块不默认共享的规则一致，未改第 1–3 章或其他正文。correctness/兼容性仅为文档目标，运行未验证，性能无结论。

#### 窗口函数调用契约

后续只交接三处窄边界：已选窗口签名/固定返回身份与分区排序/帧描述到物理计划；排名/偏移整分区访问与 last_value 帧访问到既有缓冲/流水线内核；结果到 server 两入口描述。依赖第 2 章语法/名称、第 3 章类型/比较/codec 和前块 resolver；先验证代表场景及未实现形式门禁，再单独处理 GROUPS/EXCLUDE、动态 offset 等缺口，不补建完整执行子系统，不提前生成代码任务。

来源映射：蓝图步骤 4 的 window 独立调用模型由模块链及分区/帧责任覆盖，步骤 3 全模板字段由差异表承接；同序 peer、默认/显式 ROWS/RANGE/GROUPS、空/单行、越界 offset、NULL 分别定位 WIN-CALL-PEER/FRAME/EMPTY-SINGLE/OFFSET/NULL。每组均含输入、值/NULL/类型或错误、两入口目标及证据缺口。人工审阅与首块三层身份、第 3 章固定类型、普通聚合一组一行边界一致，保留全部前序正文；correctness/兼容性仅为文档目标，运行未验证，性能无结论。

#### 有序集合聚合契约

后续只交接三处窄边界：第 2 章 AST/分析区分 WITHIN GROUP、direct 参数和排序输入，关联前块 resolver 与第 3 章类型；现有聚合入口承载每组排序状态、NULL 排除、比例校验及 final 选值/插值，不复用局部分位数合并；最终结果身份交 server 两协议描述与错误映射。依赖普通聚合的分组/资源生命周期，禁止经窗口 frame 接线。hypothetical-set 和数组比例重载须另定契约再排实施，本轮不生成代码任务。

#### 集合返回函数契约

后续仅交接三个窄边界：第 2 章 AST/分析与前块 resolver 确定调用位置、关联依赖和第 3 章固定输出身份；复用执行器承载整数序列/单数组的零至多行迭代、重启/耗尽/取消，以及单 SRF 投影展开；server 按固定列描述输出并衔接 portal 暂停恢复。多 SRF 协调、时间序列重载与复杂输出各须独立契约/证据，不递归补建子系统、不把目录适配器当通用实现，本轮不生成代码任务。

#### 章节验收与后续交接

下一步先界定“可见集合→重载/I 资格→已选签名→正式工厂/固定返回身份”的最小接线与代表拒绝证据，沿现有规范内核，不重建 PG 求值器。随后按同契约族细分 ABS/条件表达式、同名分流、时间/格式化、数组/range、系统函数和 MySQL 隔离；每族只开放已补类型、wire 与错误证据的签名。range 基础契约、name/regtype 和参数定型等前置缺口先独立确认范围，不以补齐它们扩大任何一个函数任务。

运算符、普通聚合、窗口、有序集合、SRF 各自拆窄调用模型任务，分别继承本章相应生命周期与基数断言，不纳入标量工厂补全清单。系统函数注册/求值归本章，会话事实与区域归 5，真实目录/权限事实归 6，协议/连接/取消归 7，双入口参考端采证归 1/8；实施前重新核实代码冲突、真实依赖与验证资源，不沿本轮串行文档批次直接并行编码。本轮只交接边界，不生成这些实现任务文件。

本次完成仅指第 4 章文档覆盖与验收设计闭环；来源蓝图任务保持原状态供上层独立验收。所有精确参考版本、实际构建、命令/退出码、有效断言数、原始输出、未测项和缺证字段必须在后续行为交付时补齐，文档完成不升级功能支持矩阵。

## 5. SQL会话与事务语义

### 模块

#### 会话执行链与契约模板

本块承接来源任务 `.plans/2026-10-22-postgresql18-核心兼容蓝图/5-SQL会话与事务语义.md` 步骤 1–4 的公共执行与证据边界。批次 1 的前置正文已复核：第 1 章提供能力/用例身份与证据格式，第 2 章提供连接方言及语法边界，第 3 章提供固定类型/转换契约，第 4 章提供函数契约及章节收口。来源批次 4 编号文件仍标“未开始”，但第 4 章五节及收口正文已存在，对应交付提交 `00a70c3f5a`；本块按实际正文满足文档依赖，不更新来源任务状态，不据此宣称实现完成。

以下为 2026-10-23 在基线 `9b3f6534d6b4cd328b63fcdee8aaeb774b78088d` 通过 RustCodeGraph 读取的真实符号和分派链，仅是静态证据。`pkg/server/doc.go` 不存在；模块说明和 [协议第一阶段](postgresql-protocol-first-phase.md) 的历史运行记录不代替本轮证据。

| 层与路径 | 实际符号、分派与证据边界 |
| --- | --- |
| PG 连接与共享会话 | [pg_conn.rs](../pkg/server/pg_conn.rs) 的连接协商路径调用 [conn.rs](../pkg/server/conn.rs)::SessionDriver::open_ctx，认证后选择 database，每连接创建一个 Extended。真实驱动 [runtime.rs](../pkg/server/runtime.rs)::ConcreteSessionDriver::open_ctx 返回 ConcreteTiDBContext，启动 run_session_worker，构造共享 ConcreteSession；线程名称含 mysql 不代表 PG 使用另一套引擎。 |
| 简单 Query（Q） | 同一 pg_conn.rs 循环先交 Extended::handle；Q 正常落回简单路径，清理未命名对象。先识别 SessionQuery::parse，再识别 CatalogQuery::parse_session；普通语句依次经 pg_name::adapt、pg_sql::adapt_with_context、pg_result::adapt_session_query/command，事务门禁后由 PgService::with_query 调 TiDBContext::execute_query(sql, false, …)。结果交 pg_result::write_result，末尾由 PgSession::transaction_status 提供 ReadyForQuery 状态。不能把此有界适配称作完整 PG SQL 执行支持。 |
| 扩展执行 | [pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::process 的 Parse 同样分出会话/目录查询；普通 SQL 适配后由 markers 将 $n 映射至共享参数位置，再调用 prepare_statement。Bind 解码值并保存 portal；首次 Execute 的普通路径经 pg_conn.rs 提供的闭包、with_query 到 execute_prepared_statement。portal 缓存已编码结果，后续 Execute 继续发送；这不证明执行器按需流式求值或重复副作用正确。 |
| 本地会话与目录分支 | [pg_session.rs](../pkg/server/pg_session.rs)::PgSession 保存 path、failed_transaction；SessionQuery::parse/metadata/execute 对 SHOW/SET/RESET search_path、受限 current_schema/current_database 提供专用路径。Database 分支仍委托共享 execute_query("SELECT DATABASE()")。目录查询由 CatalogQuery 执行并读取共享元数据；它们不是通用应用 DML 的替代执行器。 |
| canonical 实际接线 | conn.rs 的 TiDBContext 定义 execute_query、prepare_statement、execute_prepared_statement、in_transaction 等接口；runtime.rs 的 ConcreteTiDBContext 将前三者分别送入 SessionRequest::Execute/Prepare/ExecutePrepared。run_session_worker 在同一 ConcreteSession 上调用 execute_on_session、prepare_protocol_statement、execute_prepared_on_session。此处核实到共享会话桥接，不推断优化器/存储内部的全部 PG 契约。 |
| 两种失败状态 | PgSession::check_transaction_command/record_error/record_success 结合共享 in_transaction 管理 I/T/E；当前门禁位于简单 Query 与扩展 Parse。Extended::failed 则控制扩展消息丢弃，handle 收到 Sync 后仅清该标志，再读取事务状态。不能据此认定已准备 portal 在事务状态变化后的 Execute 全面满足门禁；后续须单独验证。 |

已读独立测试 [pg_transaction_test.rs](../pkg/server/pg_transaction_test.rs)::failed_transaction_requires_rollback 的断言意图包括简单/扩展重复键错误、25P02 门禁、Sync 后仍 E、ROLLBACK 后零行及事务内 DDL 拒绝；prepared_dml_commit_and_rollback 覆盖两连接的未提交不可见、提交后可见、回滚及断连回滚的行数断言。另读 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 的协议握手与 current_catalog 路径片段。现有行数断言不能替代下方完整值/类型断言；这些测试本轮均未执行，有效运行用例数为 0。

#### 返回子句执行契约

承接来源任务步骤 1 的 RETURNING，复用第 2 章[返回子句语法边界](#返回子句语法边界)、第 3 章[准备描述执行元数据契约](#准备描述执行元数据契约)及本章公共模板。本块分类为 PG 专属，能力 ID 为 `SESSION-RETURNING/pg`；共享执行接入不使其成为 MySQL 语义。批次 1 已交付 `d8c1ca891a110e1f57279c111176789c70cccf59`，其五节、前四章及累计前置交付已复核；不更改来源任务状态。

2026-10-23 在上述 SHA 用 RustCodeGraph 读取的静态证据如下；`pkg/server/doc.go`、`pkg/executor/doc.go` 均不存在，模块说明不充当支持证据。

| 路径与实际符号 | 源码现状、已有测试意图与限制 |
| --- | --- |
| [pg_result.rs](../pkg/server/pg_result.rs)::command | 在共享 AST 解析后检查 InsertStmt/UpdateStmt/DeleteStmt 的非空 Returning，均返回 0A000。合法 PG 形式若更早不能解析，其诊断与本门禁须分别取证，不能称全部形式都已正确拒绝。 |
| 同文件 encode_formats、write_result | 有列时编码描述和数据，CommandComplete 的 INSERT/UPDATE/DELETE 数字来自 result.state.affected_rows，而非返回行长度。类型映射、零行描述与标签编码只是输出接线，不能证明执行器产生正确行或写入。 |
| [pg_dml_test.rs](../pkg/server/pg_dml_test.rs)::prepared_crud_roundtrip | 读取完整测试：先做无 RETURNING 的参数化增查改删，再检查三类 RETURNING 在 Parse 阶段报 0A000、Sync 回 I，末尾 SELECT 无 DataRow。此时表在拒绝前已删除为空，不能证明 UPDATE/DELETE 对已有行无副作用，也没有成功返回行、生成值或回滚断言。 |
| [insert.rs](../pkg/executor/insert.rs)::InsertRuntime；[update.rs](../pkg/executor/update.rs)::UpdateRowTracker；[delete.rs](../pkg/executor/delete.rs)::DeleteRuntime | 已读现有运行时、行跟踪与删除接口片段，作为后续共享写入路径的调研边界；不从接口存在推定 PG RETURNING 已接线，也不据本次局部阅读断言整个执行器缺失能力。 |

本轮没有执行上述测试，当前运行证据为“未验证”，有效运行用例数 0；历史协议第一阶段记录仅供导航。

#### 冲突忽略执行契约

承接来源步骤 1 的 ON CONFLICT / DO NOTHING 与步骤 4 的副作用、恢复及 MySQL 共存；引用第 2 章[冲突子句语法边界](#冲突子句语法边界)，不重做 parser。能力 `SESSION-CONFLICT-NOTHING/pg` 属 PG 专属。批次 1、2 的累计依赖已由现有五节正文及提交 `d8c1ca891a110e1f57279c111176789c70cccf59`、`df9d9ebaeb` 复核；本块基于后者静态取证（2026-10-23）。

通过 RustCodeGraph 实际读取 [planbuilder.rs](../pkg/planner/core/planbuilder.rs)::PlanBuilder::buildInsert：构造 BuiltPlan::Insert 时复制 on_duplicate 与 replace；[insert.rs](../pkg/executor/insert.rs)::InsertRuntime 和 InsertExec::exec_inner：非空重复键赋值调用 batchUpdateDupRows，ignore_errors 分支调用 batch_check_and_insert，普通分支按 normal_duplicate_key_mode 写入；prefetchUniqueIndices 收集 handle/unique keys。这里是共享执行复用入口，不是 PG 冲突目标选择或错误过滤已接线的证明。

已读独立 [insert_test.rs](../pkg/executor/insert_test.rs)::insert_duplicate_check_modes_follow_transaction_and_pipeline_rules、insert_ends_snapshot_stats_when_flush_fails、insert_logs_unique_index_pointing_to_missing_row：分别断言重复键检查时机、flush 失败后的统计收尾及不一致索引错误；这些不能代替真实 PG 冲突选择与不覆盖旧行的断言。完整阅读 [pg_dml_test.rs](../pkg/server/pg_dml_test.rs)::prepared_crud_roundtrip，覆盖参数化普通 CRUD、坏 Bind 后 Sync 和 RETURNING 的 Parse 拒绝，没有 ON CONFLICT 用例。`pkg/server/doc.go`、`pkg/planner/doc.go`、`pkg/planner/core/doc.go`、`pkg/executor/doc.go` 均不存在。源码阅读与测试意图分列，实际运行用例数为 0；当前接受/拒绝阶段及诊断未验证，不能仅从局部接口缺字段断言全链能力。

#### 冲突更新执行契约

承接来源步骤 1 的 ON CONFLICT / DO UPDATE 与步骤 4 的副作用、恢复及 MySQL 共存，能力 `SESSION-CONFLICT-UPDATE/pg` 为 PG 专属。批次 3 交付 `d6f469119b` 及累计批次 1/2 的 `d8c1ca891a`、`df9d9ebaeb` 已按五节正文核对，依赖满足；复用冲突忽略块的仲裁选择和返回块的固定元数据，不改其正文。以下为 2026-10-23 在 `d6f469119b` 的静态取证，实际运行用例数 0。

通过 RustCodeGraph 实际读取 [planbuilder.rs](../pkg/planner/core/planbuilder.rs)::PlanBuilder::buildInsert，生成列处理后复制 on_duplicate/replace 到 BuiltPlan::Insert；[insert.rs](../pkg/executor/insert.rs)::InsertExec::exec_inner 按非空重复键赋值分派 batchUpdateDupRows。后者先预取冲突旧行，优先尝试 handle，再遍历 unique keys，调用 InsertRuntime::update_duplicate_row，未命中才 add_record；Open 初始化重复键求值缓冲。这些是共享执行的复用候选，不能将“任意唯一键找到首个冲突即更新”直接当作 PG 的指定仲裁。旧行与候选行绑定、独立更新条件、同语句基数保护和 PG 原子性尚无本轮全链证据；不从局部 trait 推断整个引擎必然缺失功能。

已读独立 [insert_test.rs](../pkg/executor/insert_test.rs)::insert_duplicate_check_modes_follow_transaction_and_pipeline_rules、insert_ends_snapshot_stats_when_flush_fails、insert_logs_unique_index_pointing_to_missing_row：分别断言检查模式、flush 失败统计收尾和缺失旧行索引错误，不能证明 PG 更新值、语句回滚或并发。完整阅读 [pg_dml_test.rs](../pkg/server/pg_dml_test.rs)::prepared_crud_roundtrip，普通参数化 CRUD、坏 Bind 后 Sync 及 RETURNING Parse 拒绝均有断言，没有 ON CONFLICT 成功/失败用例。pkg/server、pkg/planner/core、pkg/executor 的 doc.go 均不存在。[协议第一阶段](postgresql-protocol-first-phase.md) 的历史记录仅供入口导航；本块不声称当前支持或正确门禁。

#### 只读公共表表达式契约

承接来源任务步骤 1 的 CTE 及步骤 4 的结果、副作用、失败恢复与 MySQL 共存，能力 `SESSION-CTE-READ/pg` 为 PG 专属契约；普通查询中可能共享的子集须另有双方证据。批次 1–4 的五节正文及提交 `d8c1ca891a`、`df9d9ebaeb`、`d6f469119b`、`63b628578b43fe13461bac6a1273dfc71caaee9a` 已核对，累计依赖满足。名称绑定直接继承第 2 章[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)，不重设计 parser。

以下为 **2026-10-23** 在 `63b628578b43fe13461bac6a1273dfc71caaee9a` 的 RustCodeGraph 静态阅读；server、session、planner/core 及其 tests/cte 均无包级 doc.go。本轮没有执行 SQL 或测试，实际运行用例数 0。

| 已读源码/独立测试 | 接入责任与证据边界 |
| --- | --- |
| [pg_catalog_query.rs](../pkg/server/pg_catalog_query.rs)：`Parser::query_inner`、`Select` / `Cte` | 目录专用树以 cte_id 区分引用，先解析定义体再登记名称，检测同层重名；只接受 SELECT/WITH 定义体，拒绝 RECURSIVE，限制总计 16 个 CTE。此受限内省语法不能作为应用表查询入口，也不证明通用 CTE 执行。 |
| [pg_catalog_query_test.rs](../pkg/server/pg_catalog_query_test.rs)：`pg_introspection_cte_nesting_limits`、`pg_introspection_cte_column_bound` | 断言目录嵌套/17 个定义拒绝，以及 CTE 24 列可解析、25 列拒绝；普通目录投影不沿用同一列限额。只是解析/目录能力断言，不覆盖应用表值、事务或 PG 类型。 |
| [pg_name.rs](../pkg/server/pg_name.rs)：`Resolver::scope`；[pg_name_test.rs](../pkg/server/pg_name_test.rs)：`pg_introspection_names_token_scopes`、`pg_introspection_names_rejection_boundaries` | scope 克隆外层集合，定义体扫描后登记 CTE；RECURSIVE 返回 0A000。测试断言 CTE/public 表改写区分、嵌套范围及递归拒绝，仅为名称适配意图。应用 SQL 继续沿本章公共 Query/扩展执行链到共享会话，不能转交目录模板模拟。 |
| [logical_plan_builder.rs](../pkg/planner/core/logical_plan_builder.rs)：`buildWith`；[source.rs](../pkg/session/runtime/source.rs)：`execute_with_clause` | 前者逐项 buildCte；后者 push 作用域，按顺序求值并保存 CTE，列别名非空时要求数量与输出完全一致，body 执行后 pop。该局部实现值得复用，但其全量预求值及别名数检查不证明 PG 按需求值、部分列别名或完整类型身份；不能仅由模块并存推定所有 PG 查询均走该路径。 |
| [cte_test.rs](../pkg/planner/core/tests/cte/cte_test.rs)：`TestCTEWithDifferentSchema` | 已读独立 Rust 测试：建立用户/授权、db_a 中含 CTE 的视图，在 db_b 下 EXPLAIN，断言 CTEFullScan/seed 计划。证明现有测试意图保留 MySQL 跨数据库视图环境，不证明 PG schema 或本块结果正确。 |

#### 写入公共表表达式契约

承接来源任务步骤 1 的数据修改 CTE 与步骤 4 的副作用、恢复及 MySQL 共存，能力 `SESSION-CTE-WRITE/pg` 为 PG 专属。批次 5 提交 `aa91adacde` 及累计前置 `d8c1ca891a`、`df9d9ebaeb`、`d6f469119b`、`63b628578b` 的五节正文已核对，依赖满足。复用第 2 章 CTE 名称/语法边界、第 3 章固定列身份、第 4 章表达式资格与本章返回契约；普通应用写入仍经公共 Query/扩展入口到共享会话，不交给目录模板模拟。

2026-10-23 基于 `aa91adacde` 经 RustCodeGraph 读取的静态证据如下；`pkg/server/doc.go`、`pkg/planner/core/doc.go`、`pkg/executor/doc.go` 不存在。此表不代表运行支持或完整缺陷诊断。

| 路径与符号 | 当前证据与边界 |
| --- | --- |
| [pg_name.rs](../pkg/server/pg_name.rs)::Resolver::scope | 顺序扫描 WITH 定义体再登记名称，RECURSIVE 入口返回 0A000；只证明名称适配流程，不证明写入 CTE 可执行或共享快照。 |
| [pg_result.rs](../pkg/server/pg_result.rs)::command | 解析后只检查顶层 InsertStmt/UpdateStmt/DeleteStmt 的 Returning 并拒绝非空列表；不能把顶层检查推导为所有嵌套写入均在副作用前被拒绝。具体搬移输入能否到达此门禁、实际诊断仍未验证。 |
| [logical_plan_builder.rs](../pkg/planner/core/logical_plan_builder.rs)::buildWith；[source.rs](../pkg/session/runtime/source.rs)::execute_with_clause | 前者按定义调用 buildCte；后者建立作用域、逐项经 execute_insert_select_node_with_outer 或递归入口求值/保存、执行 body 后清理。现有行集复用值得保留，但顺序物化不能证明 PG 写入间不可见性，也未建立 DELETE 返回行生产者与外层 INSERT 同属一个原子执行的证据。 |
| [pg_dml_test.rs](../pkg/server/pg_dml_test.rs)::prepared_crud_roundtrip | 已读无返回值的 prepared DELETE 成功、三类顶层 RETURNING 在 Parse 拒绝及最终空表断言；没有本题 DELETE→INSERT 成功、嵌套拒绝或一侧失败回滚的断言。 |
| [pg_transaction_test.rs](../pkg/server/pg_transaction_test.rs)::prepared_dml_commit_and_rollback | 已读 observer 行数及 ROLLBACK/断连回滚断言片段，可复用双连接测试形式，不能替代本题两个表的完整行值、同语句快照及原子性证据。 |

以上独立测试本轮均未运行；有效运行用例数 0，当前支持为未验证，不以说明文件或历史协议记录升级结论。

#### 模式与搜索路径契约

承接来源任务步骤 2 的 schema/search_path 与步骤 4 的隔离、失败恢复及 MySQL 共存，能力 `SESSION-SEARCH-PATH/pg` 为 PG 专属。批次 6 提交 `aef96fb4fe` 及累计前置 `d8c1ca891a`、`df9d9ebaeb`、`d6f469119b`、`63b628578b`、`aa91adacde` 的五节正文已核对，文档依赖满足。复用第 2 章[模式搜索路径解析契约](#模式搜索路径解析契约)，此块主责连接状态与事务生命周期；仍通过公共 PG 入口进入共享会话/执行链。

2026-10-23 基于 `aef96fb4fe` 经 RustCodeGraph 阅读的静态证据如下；`pkg/server/doc.go` 不存在。说明文件及[协议第一阶段](postgresql-protocol-first-phase.md)的历史结果只作导航。

| 路径与符号 | 现状与证据限制 |
| --- | --- |
| [pg_session.rs](../pkg/server/pg_session.rs)::PgSession::{default,execute,schema,has_public,public_precedes_catalog}、SessionQuery::parse | 默认仅 public；SET 克隆替换 path，SHOW 拼接原列表，schema 取列表首项；RESET 直接重建整个 PgSession。仅接受 public、pg_catalog、空字符串与无重复列表，missing/任意用户 schema 和重复项在此返回 0A000。未表示事务前值、LOCAL 覆盖或回滚恢复；RESET 同时重建 failed_transaction 的代码不能充当失败事务恢复证明。 |
| [pg_name.rs](../pkg/server/pg_name.rs)::Resolver::relation、rewrite、adapt | 未限定非 CTE 关系仅消费 has_public；显式 public.t 改写当前原生库，adapt 通过共享 SELECT DATABASE() 取库，再校验关系；其他 schema/跨库拒绝。public 是当前库投影，不是原生 public 库；pg_catalog 是有界 PG 目录入口，不是任意原生库映射。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::CatalogQuery::parse_session_with_types | 仅 public 显式先于 pg_catalog 时读真实当前库快照检查同名表遮蔽；current_schema 取 session.schema()。此处不是通用多 schema resolver，路径排除与目录行消失也不是一回事。 |
| [pg_session_test.rs](../pkg/server/pg_session_test.rs)::pg_introspection_namespace_parser_boundaries；[pg_name_test.rs](../pkg/server/pg_name_test.rs)::pg_introspection_names_token_scopes、pg_introspection_names_rejection_boundaries | 已完整阅读独立测试：有界 SET/SHOW/RESET 解析、显式 public 绕过成员门禁、CTE 遮蔽及跨库/非 public 拒绝断言；不含事务路径恢复证明。 |
| [pg_query_test.rs](../pkg/server/pg_query_test.rs)::pg_introspection_namespace_session_state、pg_introspection_names_live_tables | 已读双连接路径交错、空路径 current_schema NULL、隐式目录、RESET、Parse 后 SET 再读取 current_schema，以及真实表/当前库隔离的断言。现有测试意图不能证明任意 schema、权限、完整 catalog 遮蔽或普通 prepared 关系重绑定。 |

本轮以上测试均未运行，实际运行用例数 0；当前有界实现、既有测试意图、以下完整目标和未来运行证据分别记录，不宣称功能验收通过。

#### 引号与大小写执行契约

承接来源步骤 2 的引号/大小写与步骤 4 的实际副作用、恢复和 MySQL 共存，能力 ID 为 `SESSION-NAME-CASE/pg`，复用第 2 章[引号与大小写规则](#引号与大小写规则)、[限定名称解析规则](#限定名称解析规则)及本章路径块，不重新设计 lexer。批次 7 提交 `39f6e32b84ff1c323099507d6d75cc7512179210` 及累计批次 1–6 的五节正文已复核，依赖满足；总计划与来源任务只读。

2026-10-23 通过 RustCodeGraph 读取 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Token`、`source_tokens`、`Resolver::relation`、`rewrite`、`adapt`、`validate_relations`：未引号 token.name 转小写，双引号名称解码且保真；关系位置按分段改写当前原生库，双引号转为后端反引号，未编辑文本原样保留。`adapt` 读共享 `SELECT DATABASE()` 后二次改写，`validate_relations` 只校验共享 AST 暴露的 TableName.Schema.L 是否越库，不提供关系/列/别名的 PG 精确身份绑定证明。该链仍复用共享 TiDBContext；没有第二套 PG 存储或执行器。

已完整读独立 [pg_name_test.rs](../pkg/server/pg_name_test.rs)::`pg_introspection_names_token_scopes`（public/当前库限定、引用名称与字符串/注释保护的改写断言）、`pg_introspection_names_rejection_boundaries`（跨库、非 public、空/未闭合引号拒绝）及 [pg_session_test.rs](../pkg/server/pg_session_test.rs)::`pg_introspection_namespace_parser_boundaries`（有界路径解析及引用 PUBLIC 拒绝）。这些测试没有以不同对象值证明 t/"T"、同名大小写列或别名绑定正确；本轮未执行，运行用例数 0。`pkg/server/doc.go` 不存在；模块说明和协议第一阶段的历史支持叙述不是本轮运行证据。

#### 预处理执行时会话契约

承接来源步骤 2 的 prepared 执行时状态及步骤 4 的隔离/恢复，能力 ID 为 `SESSION-PREPARED-STATE/pg`。批次 8 提交 `3ba468a243`、批次 7 `39f6e32b84` 及累计批次 1–6 的五节正文与提交链已核对，满足文档依赖；第 2 章名称、第 3 章固定类型、第 4 章函数身份契约继续适用，总计划及来源任务保持只读。

2026-10-23 基于 `3ba468a243` 通过 RustCodeGraph 阅读 [pg_extended.rs](../pkg/server/pg_extended.rs)::Statement/Portal/Extended::process/handle：普通 Parse 在当时 PgSession 上调用 pg_name::adapt，再经 markers 和共享 prepare_statement 保存原生 handle、参数 OID/位置映射及准备元数据；Bind 解码值并复制列/类型与描述到 portal。Describe statement 返回已保存参数/列，Describe portal 返回其描述；首次 Execute 调共享 execute_prepared_statement，编码后与 portal 描述比较，不一致返回产品 0A000；之后 Execute 使用 messages/offset 缓存。此比较发生在执行后，不能据此证明写入前已完成固定结果类型检查，也未在该边界找到普通语句重新进行 PG 名称适配的证据；不据此断言共享引擎完全没有 DDL 失效处理。

[pg_session.rs](../pkg/server/pg_session.rs)::PgSession::execute 的 Schema/Show 在执行时读本连接路径；[pg_conn.rs](../pkg/server/pg_conn.rs) 每连接创建 Extended，其扩展闭包将目录交 CatalogQuery::execute、普通 handle 交同一 TiDBContext。[pg_catalog.rs](../pkg/server/pg_catalog.rs)::CatalogQuery/Execution 承载目录解析与单次执行快照；目录 statement 不分配原生 prepared handle，重新读取目录行不等于普通关系/函数重新绑定，public_first 等解析状态也不能假定全部在 Execute 更新。

已读独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 的 current_catalog、显式 int4 参数/两种 Describe、Parse 后建库可见和目录 statement Close 隔离片段；[pg_query_test.rs](../pkg/server/pg_query_test.rs)::pg_introspection_namespace_session_state 的两连接路径及 Parse 后 SET/current_schema 断言；[pg_dml_test.rs](../pkg/server/pg_dml_test.rs) 的 prepared CRUD、重复/乱序参数与非法 Bind 后 Sync 断言。均仅是测试意图，本轮未运行；它们不覆盖下面的普通 prepared 路径切换、跨连接 DDL 和固定结果类型全序列。`pkg/server/doc.go` 不存在，历史说明不作为本轮支持证据。

#### 序列执行契约

承接来源步骤 2 的 sequence 及步骤 4 的副作用/恢复，能力 ID 为 `SESSION-SEQUENCE/pg`。批次 9 提交 `3fffa00db6` 及累计批次 1–8 的提交链、五节正文已核对，满足文档依赖；第 2 章名称、第 3 章 regclass/bigint、第 4 章函数身份继续适用。以下是 2026-10-23 基于该提交的 RustCodeGraph 静态阅读，不是运行证明。

[pg_catalog.rs](../pkg/server/pg_catalog.rs)::native_metadata_rows 仅在真实 model.Sequence 存在时投影 pg_sequence 的起点、步长、边界、缓存和循环参数；目录行不执行 nextval/currval/setval。[autoid.rs](../pkg/meta/autoid/autoid.rs)::DefaultAllocator::alloc_seq_cache 在 IdStore 事务中分配序列缓存水位，rebase_seq 已满足水位时返回 already_satisfied；它不承载 PG setval 的 is_called 参数，不能以 rebase 等价实现 PG 任意回设。

[数据库信息函数内核](../pkg/expression/builtin_info.rs)::SequenceService/next_val/last_val/set_val 是共享原生接口：next_val 按序列 ID 更新 SessionInfo.sequence_state，last_val(source) 无本连接值时返回 None；权限为原生 Insert/Select，set_val 无布尔参数且接收不可变 SessionInfo。这与 PG currval 未初始化报错、PG 权限和 setval 更新连接状态的目标不同，仅证明可复用入口，未证明 PG 签名注册、工厂接线、regclass 解析和事务非回滚已经完整通过。

独立 [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_dependencies_live 建真实序列和 AUTO_INCREMENT 表，断言后者不是序列、无 owned-by 列依赖，以及默认值序列引用被拒绝；[seq_autoid_test.rs](../pkg/meta/autoid/seq_autoid_test.rs)::test_sequence_autoid/test_concurrent_alloc_sequence 用 MemoryStore 检查缓存/循环与并发分配不重复。测试意图不覆盖 PG 应用 SQL、真实持久化、两连接 currval 或 ROLLBACK。本轮上述测试未运行，有效运行用例数为 0；server、expression、meta/autoid、executor 的 doc.go 均不存在。

#### 标识列生成契约

承接来源任务步骤 2 的 identity、步骤 4 的副作用/恢复，能力 ID 为 `SESSION-IDENTITY/pg`，分类 PG 专属；关联 `SESSION-SEQUENCE/pg` 与本章 RETURNING 契约。批次 10 提交 `b6ada3e90e6dfecb0df5f4f3d394fcc12f49f475` 及累计批次 1–9 的提交链和五节正文已核对，满足文档依赖。以下为 2026-10-23 基于该 SHA 的静态阅读；model、executor、server 均无 doc.go。

| 已读真实路径/符号 | 当前证据与限制 |
| --- | --- |
| [column.rs](../pkg/meta/model/column.rs)::ColumnInfo；[table.rs](../pkg/meta/model/table.rs)::SequenceInfo | 列保存 DefaultValue、DefaultIsExpr、GeneratedExprString、GeneratedStored、FieldType；已读完整字段未见 ALWAYS/BY DEFAULT 模式及所拥有序列的稳定引用。SequenceInfo 只有起点、缓存、边界、步长、循环和注释，无所属表/列。表达式生成列、AUTO_INCREMENT 与 identity 是不同身份。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::column_catalog_rows、native_metadata_rows、sequence_dependency_rows | 列目录当前将 attidentity 与 attgenerated 投影为空；pg_sequence 只投影真实 model.Sequence；依赖仅为真实序列到 schema 的 n 边，没有列拥有关系。图对 native_metadata_rows 仅返回签名，已补读函数体；不能按目录列存在宣称 identity 已支持。 |
| [insert.rs](../pkg/executor/insert.rs)::InsertRuntime | 已有 insert_rows/add_record、auto_increment_column 等共享写入接口，不能从接口或 auto-ID 提示推定 PG identity 模式、覆盖子句和返回生成值已接线；后续沿同一规范执行链接入。 |
| 独立 [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_dependencies_live | 真实序列与 AUTO_INCREMENT 表分开建模，断言后者不是序列、owned-by 列依赖查询为空，默认值序列引用创建被拒绝。此为现有拒绝/目录测试意图，没有 identity 成功插入证据。 |
| 独立 [pg_dml_test.rs](../pkg/server/pg_dml_test.rs)::prepared_crud_roundtrip；[insert_test.rs](../pkg/executor/insert_test.rs)::ordinary_auto_id_error_still_uses_statement_error_context | 前者验证无 identity 的 prepared CRUD，并断言 RETURNING 在 Parse 拒绝；后者验证原生 auto-ID 错误进入语句错误处理，不证明 PG 生成规则。 |

这些源码/测试仅供现状和后续复用判断，本轮均未运行，有效运行用例数 0；协议阶段的历史结果不升级为本轮支持证据。

#### 事务提交与回滚契约

承接来源任务步骤 3 的 BEGIN/COMMIT/ROLLBACK 和步骤 4 的副作用、恢复与 MySQL 共存；能力 `SESSION-TXN/pg` 与 `SESSION-TXN/mysql` 关联，正常 DML 提交/撤销分类为共享语义，PG 的 I/T/E、警告和失败门禁单独分类 PG 专属。批次 11 提交 `3922401fa4a2cbe397eba1b45aa64422714817ef` 及累计批次 1–10 连续提交和五节正文已核对，满足本块文档依赖。以下为 2026-10-23 的静态证据；server、session、session/runtime、sessiontxn 均无 doc.go。

| 已读路径/符号 | 当前事实及证据限制 |
| --- | --- |
| [pg_session.rs](../pkg/server/pg_session.rs)::PgSession::transaction_status、check_transaction_command、record_error、record_success | 共享 context.in_transaction=false 时清失败标志并返回 I；共享事务开启时由连接私有 failed_transaction 区分 T/E。错误只在底层仍处于事务时置失败；当前 E 只放行 ROLLBACK，连 COMMIT 也返回 25P02。此为现状，不将这种 COMMIT 策略称为完整 PG 等价。 |
| [runtime.rs](../pkg/server/runtime.rs)::ConcreteTiDBContext::in_transaction、run_session_worker、execute_on_session | in_transaction 读取共享状态位 0x0001；worker 创建 ConcreteSession，普通执行走 session.execute，prepared 走 execute_prepared_on_session。Shutdown 退出 worker；接线和状态位本身不是跨连接可见性、关闭清理完成或 PG 隔离模式的证明。 |
| [session.rs](../pkg/session/session.rs)::session::doCommit、RollbackTxn、Close；[txnmanager.rs](../pkg/session/txnmanager.rs)::TxnContextProvider；[interface.rs](../pkg/sessiontxn/interface.rs)::TxnContextProvider | session 层提交委托 runtime，RollbackTxn 调有效事务的 Rollback，Close 调回滚并继续释放资源；provider 暴露语句读 TS、快照和语句提交/回滚钩子。这些抽象不能替代 ConcreteSession 实际分派和存储运行证据，不能从类型存在推定 PG RC/RR/Serializable 已接线。 |
| 独立 [pg_transaction_test.rs](../pkg/server/pg_transaction_test.rs)::prepared_dml_commit_and_rollback | 真实 CreateAnalyzeSession、两 TCP 连接；prepared INSERT 在提交前观察零行、提交后一行、第二次 INSERT 回滚后仍一行、第三次 INSERT 后 Terminate/关闭仍一行。断言主要数 DataRow，未核对 UPDATE/DELETE 撤销后的完整值，亦不覆盖异常断网或崩溃持久性。 |
| 同文件 ::failed_transaction_requires_rollback | 简单重复键后 E，SELECT/COMMIT 为 25P02；扩展重复键为 23505，Sync 仍 E，ROLLBACK 后 I 且零行；事务内 DDL 的 0A000 是产品限制。本块仅引用恢复交界，不吞并后续失败事务、Sync 或 DDL 契约。 |

上述测试本轮未执行，有效运行用例数为 0；协议第一阶段的历史运行记录与模块说明不升级为当前支持证据。

#### 失败事务恢复契约

承接来源任务步骤 3 的错误事务状态与恢复、步骤 4 的副作用与 MySQL 共存，复用本章会话执行链和事务提交与回滚契约；能力 `SESSION-TXN-FAILED/pg` 属 PG 专属失败态策略，共享事务仍由同一 TiDBContext/会话/存储负责。批次 12 提交 `7da3761700` 及历史中批次 1–11 的连续文档提交、五节累计正文已复核，文档前置满足；编号文件移除不作为功能通过证据。2026-10-23 在该基线通过 RustCodeGraph 实际读取以下符号；`pkg/server/doc.go` 不存在。

| 实际路径与符号 | 当前静态行为与证据边界 |
| --- | --- |
| [pg_session.rs](../pkg/server/pg_session.rs)::transaction_status | 共享 in_transaction=false 时清 failed_transaction 并返回 I；否则按失败标志返回 E 或 T。该方法观察并更新局部标志，不负责回滚底层写入；底层提前结束事务时是否仍满足 PG 显式失败块，须独立验证。 |
| 同文件 check_transaction_command | 无共享事务时清失败标志并放行；有事务且失败时仅 command=ROLLBACK 放行，其余返回 25P02，包括 COMMIT。健康显式事务还拒绝所列 DDL（0A000）。只检查命令分类，不能据此证明 ROLLBACK TO、ABORT 等变体完整支持。 |
| 同文件 record_error / record_success | record_error 仅在共享事务仍活动时置失败；record_success 在命令分类为 ROLLBACK 或共享事务已结束时清失败。成功普通查询本身不清失败，清标志也不能代替真实回滚。 |
| [pg_conn.rs](../pkg/server/pg_conn.rs)::PgService 的 Q 分派 | 普通执行前调用门禁，解析/适配和引擎执行失败分支调用 record_error，成功执行调用 record_success，最后从 transaction_status 输出状态。会话专用查询也有门禁；不从局部路径推断所有响应编码/连接错误都统一处理。 |
| [pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::process / handle | Parse 调门禁，process 错误令 Extended.failed=true 并 record_error；Sync 清协议 failed 后读取事务状态，不执行显式事务 ROLLBACK。首次 Execute 直接执行 portal 的会话/目录或共享 prepared 分支，未重新调用该门禁；已准备/绑定对象跨失败态的执行仍需补证，不能宣称全部读写已被拦截。 |

已完整阅读独立 [pg_transaction_test.rs](../pkg/server/pg_transaction_test.rs)::failed_transaction_requires_rollback：Q 重复键后 E，SELECT/COMMIT 为 25P02，ROLLBACK 后零行；扩展重复键 23505、Sync 后 E，新 Parse 被 25P02 拒绝。它不覆盖失败前准备的 portal 在失败后 Execute、除零、失败态 COMMIT 的 PG 参考响应或跨连接失败标志隔离。[pg_error_test.rs](../pkg/server/pg_error_test.rs)::sqlstate_unique_syntax_and_recovery 断言事务外唯一键/语法/多语句拒绝后 SELECT 1 的完整值；unknown_errors_are_not_misclassified 保留未知错误 XX000，不把任意字符串映射为已知诊断。测试意图、源码现状与运行证据分开；本轮均未运行，有效运行用例数 0。[协议第一阶段](postgresql-protocol-first-phase.md) 的历史运行记录不升级为本轮验证。

#### 协议同步与事务恢复边界

承接来源任务步骤3的“Sync与事务恢复的区分”和步骤4的失败后继续性，关联 `SESSION-TXN-FAILED/pg`，本题用例族为 `session.txn.sync.*`；协议同步为PG专属，真实事务仍由共享会话/执行/存储负责。前置批次13提交 `bd8f486b17858b702b105c7e939e4cb17a677c53` 及累计1–12的连续提交与五节正文已核对，满足文档依赖；任务文件删除不作为功能证据。以下为2026-10-23经RustCodeGraph读取的静态事实，`pkg/server/doc.go` 不存在。

| 实际路径/符号 | 当前事实与证据边界 |
| --- | --- |
| [pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::handle | failed且非Sync时直接丢弃，连Q也不分派；process错误置协议failed并调用PgSession::record_error。合法Sync清协议failed，底层无事务时清portals，随后读取transaction_status发ReadyForQuery。该分支无共享事务提交/回滚调用，不能证明无BEGIN流水线的整段原子性；也不能把清portal当作回滚。 |
| 同文件 ::process 的 Parse、Bind、Execute | Parse调用事务门禁；Bind保存portal，Execute首次执行后缓存编码消息。Execute未重新调用事务门禁，提前绑定对象跨E执行和缓存续发均须单独取证；此为风险入口，不预判实际运行结果。 |
| [pg_session.rs](../pkg/server/pg_session.rs)::transaction_status、record_error、record_success、check_transaction_command | 根据共享in_transaction及连接failed_transaction合成I/T/E；共享事务活动时record_error置失败，当前E只放行ROLLBACK。Sync不调用record_success。共享事务提前结束时局部标志被清，是否满足显式失败块仍须行副作用与状态联合证明。 |
| [pg_conn.rs](../pkg/server/pg_conn.rs)::PgService连接循环 | 每连接仅一个Extended，其session同时供Q和扩展路径使用；Q在Extended放行后才调用共享execute_query，扩展闭包调用execute_prepared_statement。Terminate先于扩展分派处理；不能把致命连接错误列为可由Sync恢复。 |
| 独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync | 已读成功Parse/Bind/Execute/Sync，以及缺OID的Parse拒绝0A000后发送Q DELETE被丢弃、Sync后表仍两行、后续布尔查询成功的断言。它不是PG参考语法错误用例，也不证明此前多条成功写入整段回滚。 |
| 独立 [pg_transaction_test.rs](../pkg/server/pg_transaction_test.rs)::failed_transaction_requires_rollback | 已读扩展重复键23505、Sync后E、新Parse被25P02拒绝及Q ROLLBACK后I/零行；缺Bind错误、取消、预绑定portal、无Sync多写与跨连接完整值断言。 |

上述测试本轮均未运行，有效运行用例数0。[协议第一阶段](postgresql-protocol-first-phase.md)为历史边界参考，不是本轮支持证据。

#### 事务型建表能力门槛

本块承接来源任务步骤 3–4 的事务型 DDL 原型与底层限制，能力 `SESSION-TX-DDL/pg` 为 PG 专属目标，关联 `SESSION-TX-DDL/mysql` 的原生共存契约。批次 14 提交 `e8b9292494`、13 的 `bd8f486b17`、12 的 `7da3761700` 及累计 1–11 顺序提交与五节正文已核对，依赖满足。复用第 2 章名称/语法、第 3 章固定描述、第 4 章表达式及前块 I/T/E、失败恢复和 Sync；继续共享 TiDBContext、规范会话、元数据与存储，不另建 PG 引擎。

2026-10-23 在上述批次 14 基线上经 RustCodeGraph 定点读取以下静态证据；先读 [DDL 入口](agents/ddl/README.md) 和 [doc.go](../pkg/ddl/doc.go)，server、session、executor 无包级 doc.go。运行测试数为 0；[协议第一阶段](postgresql-protocol-first-phase.md) 是历史说明，不能提升本块的证据等级。

| 路径与实际符号 | 现状、断言意图与边界 |
| --- | --- |
| [pg_session.rs](../pkg/server/pg_session.rs)::PgSession::check_transaction_command、record_error、record_success、transaction_status | 共享 context.in_transaction 为真时，失败态先拒绝除 ROLLBACK 外的命令为 25P02；正常态对 CREATE TABLE、DROP TABLE、CREATE DATABASE、DROP DATABASE、ALTER TABLE、TRUNCATE TABLE 六类命令返回产品限制 0A000。record_error 在活跃事务设置失败标志，状态由共享事务和该标志组成 I/T/E；不是 PG 对所有 DDL 的规则。 |
| [pg_result.rs](../pkg/server/pg_result.rs)::command | DropTableStmt.IsView 单独映射 DROP VIEW，不在上述六类集合；其他未知命令可能先在 command 被拒绝。DROP VIEW 只能列为门禁覆盖/事务副作用调查缺口，不能因未列而声明可事务；同理不得推断 CREATE VIEW、索引、序列等命令支持。 |
| [pg_transaction_test.rs](../pkg/server/pg_transaction_test.rs)::failed_transaction_requires_rollback、prepared_dml_commit_and_rollback | 前者断言 BEGIN 后 CREATE TABLE 返回 0A000、ReadyForQuery E，再 ROLLBACK→I；另有 DML 错误、25P02 和恢复断言。后者断言两连接 prepared DML 的提交可见、回滚与关闭后不多行。没有六类 DDL 的完整副作用矩阵、事务建表成功或元数据原子性断言；本轮只读未运行。 |
| [ddl.rs](../pkg/executor/ddl.rs)::DDLExec::Next、DdlRuntime；[runtime/ddl.rs](../pkg/session/runtime/ddl.rs)::ConcreteSession | 前者先 new_transaction_in_statement，再 dispatchDDL，成功后 refresh_transaction_infoschema、leave_transaction；后者承载 DDL 元数据与 job 操作。泛型执行器的存在及局部 runtime 阅读不证明 PG 工厂完整接线或 DDL 能加入用户事务。DDL 入口所述 job/owner、schema version/sync、MDL 与持久化均是后续原型须逐项实证的依赖，不能把 DDL job 自身回滚当用户事务回滚。 |

#### 章节验收与后续交接

本块收束来源任务步骤 1–4 的契约文档，采用第 1 章证据格式、第 2 章语法/名称、第 3 章固定类型及第 4 章求值契约。2026-10-23 已核对批次 1–15 的连续文档提交（`d8c1ca891a110e1f57279c111176789c70cccf59` 至 `6f40e5d825cd930007d7efc40d4c1444f7c8b7f9`）与五节各十五块正文；批次 15 及累计前置满足的是文档依赖。编号文件是否存在不作完成证据；不修改来源任务状态或整体计划。

在上述批次 15 基线通过 RustCodeGraph 重新读取 `PgSession::{transaction_status,check_transaction_command,record_error,record_success,execute}`、`Extended::{handle,process}`、`Statement`/`Portal`、`pg_result::command` 与 `pg_name::source_tokens`。实际路径分别为 [pg_session.rs](../pkg/server/pg_session.rs)、[pg_extended.rs](../pkg/server/pg_extended.rs)、[pg_result.rs](../pkg/server/pg_result.rs)、[pg_name.rs](../pkg/server/pg_name.rs)。独立 [pg_transaction_test.rs](../pkg/server/pg_transaction_test.rs) 的 `failed_transaction_requires_rollback` / `prepared_dml_commit_and_rollback` 与 [pg_dml_test.rs](../pkg/server/pg_dml_test.rs) 的 `prepared_crud_roundtrip` 已定点复读：现有断言意图分别覆盖失败态/DDL拒绝、双连接行数和普通 prepared CRUD/非法 Bind。它们没有提供本章全部目标的成功证据，本轮执行测试数为 0。其余族的静态入口及已读测试仍以各块“模块”表为准，不把目录测试当作应用 SQL 支持证据。

本次复核仍见：失败态 COMMIT 被 25P02 拒绝；扩展 Parse 有事务门禁，首次 Execute 未重新调用该门禁；Sync 清协议 failed 并读取事务状态，没有在该分支提交/回滚共享事务；RESET 重建 PgSession，没有证明事务路径恢复；普通 Parse 固定适配后的 SQL，Bind 保存描述，Execute 才运行共享 handle。前述事实是局部源码现状，分别留下失败态出口、旧 portal、隐式事务段、路径恢复和普通 prepared 重绑定的验证入口，不推断所有底层分支的运行结果。六类事务内 DDL 拒绝和顶层 RETURNING 拒绝同样只作静态限制。`pkg/server/doc.go` 不存在；已读 [DDL 入口](agents/ddl/README.md) 与 [pkg/ddl/doc.go](../pkg/ddl/doc.go)，不把其框架描述升级为 Rust 用户事务原子性证明。

### 目标行为

#### 会话执行链与契约模板

Q 与扩展 Execute 应到达同一所属连接的规范会话，使用相同的名称、类型、函数与事务契约；局部会话查询必须服从适用的事务门禁。共享执行、返回 CommandComplete、Parse 成功或内省 SELECT 成功都不是副作用正确的充分证据。保留第 1 章四种分类；能力用稳定 `SESSION-…/pg`、关联 `SESSION-…/mysql`，用例用唯一 `session.…`，两方言经 related_case_ids 关联，不能将场景族名直接当单个测试通过记录。

| 必填字段 | 本章统一记录规则 |
| --- | --- |
| 身份及归属 | capability_id、case_id、related_case_ids、分类、所属工作流与协作方；稳定 ID 是文档契约身份，不预设 Rust API。 |
| 输入及前置数据 | 方言 SQL/消息顺序、参数位置/值/类型/OID/格式、两端各自 setup_sql、cleanup_sql；连接/数据库、编码/时区、事务初始状态、autocommit、隔离级别、会话设置及并发观察时点，禁止隐含准备。 |
| 目标与当前状态 | 分列官方目标、AsterSQL 允许的暂时限制、源码现状、独立测试断言意图与实际运行 observations；当前证据缺失写“未验证”，不将未实现自动归为明确不支持。 |
| 返回类型与行 | 成功结果列序/列名、逻辑类型、可空性、PG OID/typlen/typmod/格式或 MySQL 类型标志、值/SQL NULL/零行、排序及重复行；无结果集写原因。CommandComplete 标签与影响行数另存，不冒充返回行。类型转换和 NULL 表示沿第 3、1 章。 |
| 副作用 | 每步写入前后本连接、另一连接的完整 SELECT 输入与预期数据；提交/回滚/拒绝/断连后检查对象与数据是否变化。解析或响应成功不替代持久化、可见性及回滚证明。 |
| 错误 | 成功写“不适用（成功）”；拒绝写来源、发生阶段、SQLSTATE 及 S/V/C/M、适用 D/H/P，精确消息/位置未采就明示。AsterSQL 能力限制与参考 PG 错误分开，MySQL 自身 code/SQLSTATE/警告另存，禁止从错误文本猜码。 |
| 失败后可继续性 | 错误后流水线丢弃状态、ReadyForQuery I/T/E、仍允许/禁止的操作、Sync 与 SQL 恢复命令的顺序、恢复后 SELECT 和副作用；无显式事务与 BEGIN 内分别记录，不从前者推断后者。 |
| MySQL 对照 | 原 MySQL Query、Prepare/Execute、sql_mode、数据库/事务语义及元数据各对自己的参考端；PG 专属语法不用 MySQL 近似语法代替，产品限制不用 PG 参考成功伪造等价通过。 |
| 证据及后续依赖 | 沿第 1 章 format_version、dialects、comparison、observations、evidence_refs、verdict 格式；绑定源码 SHA/符号/独立测试、官方版本/URL/访问日，运行时补精确构建/参考端版本、完整命令/退出码、有效用例数与原始输出位置。列明未取证项、依赖及能力开放门槛；空白或裸 null 不是缺证标记。 |

责任边界：第 2 章负责所属连接方言、PG 语法/AST、占位符及原 SQL 诊断位置；第 3 章负责分析时固定类型、转换资格与协议类型身份；第 4 章负责函数/运算符名称、签名与求值。第 5 章只组合应用语句的行、副作用、会话与事务状态；工作流 6 提供真实对象、名称与目录事实，不用虚构 OID/XID/WAL 补缺；工作流 7 提供消息格式、Describe/codec、取消与连接生命周期，将本章语义状态编码到 wire。第 8 章消费双方言共存证据。后续接线继续共享存储、优化器与执行器。

本块官方依据均于 **2026-10-23** 访问，适用 **PostgreSQL 18 大版本手册**：[§54.2.2–3 Query/扩展消息流程](https://www.postgresql.org/docs/18/protocol-flow.html)用于区分处理阶段、Describe 与同步恢复；BEGIN 内的 Sync 不结束显式事务。隐式事务在 Sync 边界的行为须另验，不能由当前连接标志推定。[§3.4 Transactions](https://www.postgresql.org/docs/18/tutorial-transactions.html)用于提交可见性、回滚及失败恢复目标；savepoint 恢复另交后续细分。[INSERT](https://www.postgresql.org/docs/18/sql-insert.html)用于实际写入与 RETURNING 行区分，返回行专项由后块定义。[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)核对 23505、25P02、0A000 的名称；其中 AsterSQL 的事务内 DDL 0A000 是产品限制，不能外推为 PG 对普通 CREATE TABLE 的规范拒绝。**18.6** 仍仅为来源指定目标，未核实该补丁发布或采集其参考运行结果，以上链接不能证明精确补丁行为。

#### 返回子句执行契约

三类语句必须从同一次真实写入取得返回行：INSERT 默认返回插入后含默认值的行，UPDATE 默认返回修改后的行，DELETE 默认返回删除前的行；每个范围内实际处理的目标行产生对应投影，零匹配仍有固定结果列但无 DataRow。不通过执行第二次写入、回查猜测旧值或改 CommandComplete 构造成功。返回的投影可以与存储值不同，影响行数与返回行数分别断言；普通夹具无规则/触发器，UPDATE 将值设为原值也应计入匹配更新，不能套用 MySQL changed-rows 口径。多行 RETURNING 无隐含顺序保证，以带主键的多重集比较，不能只比行数或排序后丢弃重复值。

有序投影、星号展开、别名与引号沿第 2 章；目标表别名参与绑定，输出别名不能改存储列。类型由第 3 章的列与表达式分析确定，不能由首行、NULL、影响行数或标签推定。下方 id/v/算术投影为 int4（OID 23、typlen 4、typmod -1），note 为 text（OID 25、typlen -1、typmod -1），均用 text 格式；SQL NULL 以长度 -1 编码，与空字符串区分。SQL 可空值与协议 RowDescription 字段分开记录，不发明协议可空性位；直接列来源身份与表达式来源按第 3、6、7 章取证。Parse/Describe 不执行写入，statement/portal Describe 与 Execute 的类型及列序一致；Bind 换值或 typed NULL 不改变描述。

先在副作用前拒绝尚未开放的合法形式（AsterSQL 限制目标 0A000），包括不能完整承载或求值的返回投影；不得删去 RETURNING 后提交写入。语法错误、绑定错误、运行时表达式错误分阶段记录，不能一律报语法错误；运行中失败须保证整条语句无部分持久写入，不能把错误前收到的 DataRow 当作提交证明。显式事务失败进入 E，Sync 仅恢复消息流水线，ROLLBACK 后才回 I；普通成功的返回行也须另证 COMMIT/ROLLBACK 副作用。

官方依据于 **2026-10-23** 查阅，适用 **PostgreSQL 18**：[§6.4](https://www.postgresql.org/docs/18/dml-returning.html)定义三类默认行镜像与输出投影；[INSERT](https://www.postgresql.org/docs/18/sql-insert.html)定义默认值、输出表达式及实际插入/更新行；[UPDATE](https://www.postgresql.org/docs/18/sql-update.html)定义新值、别名和包含值未改变行的计数；[DELETE](https://www.postgresql.org/docs/18/sql-delete.html)定义被删行和计数。[§54.7](https://www.postgresql.org/docs/18/protocol-message-formats.html)用于结果消息与 CommandComplete 分责，[§3.4](https://www.postgresql.org/docs/18/tutorial-transactions.html)用于回滚及可见性，[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)用于 0A000/25P02/22012 错误类别。以上为目标依据；虽当前附录页面提及 18.6，仍无该补丁版本的参考实例、发布核验或实测结果，不将大版本手册升级为精确补丁运行证据。

#### 冲突忽略执行契约

复用本章公共模板及 RETURNING 契约。DO NOTHING 对所选仲裁冲突跳过该候选行，不能更新、删除或覆盖已有行；无冲突行仍正常写入。目标选择必须来自真实唯一索引/约束元数据，禁止先 SELECT 再 INSERT 的竞态近似或 MySQL INSERT IGNORE 等价替换。

| 目标形态 | PG 目标与产品开放条件 |
| --- | --- |
| 无目标 `ON CONFLICT DO NOTHING` | 处理所有可用仲裁约束/唯一索引的冲突；不是吞掉所有错误。 |
| 列/表达式目标 `(id)` | 按第 2 章保真信息推断匹配的唯一索引；不是任意唯一键都可忽略。无匹配索引须报错，即使候选行本可无冲突写入。 |
| `ON CONSTRAINT conflict_doc_pkey` | 名称绑定到本表真实可用约束，不将普通索引名冒充约束名。不存在或不可用须失败。 |
| 部分索引 `(k) WHERE active` | 谓词用于索引推断，不是候选行过滤；匹配的非部分唯一索引也可能被推断。必须证明推断及索引适用行，否则该合法形态在写入前明确能力拒绝。 |

只有适用的非延迟唯一/排斥仲裁冲突可由此动作处理；列推断针对唯一索引，非延迟排斥约束仅列为 DO NOTHING 的后续独立能力门槛，不从 DO UPDATE 支持与否类推。NOT NULL、CHECK、外键、类型转换、权限、表达式求值、存储及非仲裁唯一键错误均不得笼统降成警告。遇到多种同时违规不承诺固定检查顺序；验收用隔离单一错误的输入。尚未证明的合法目标要求 0A000 能力拒绝，实际门禁仍待取证；这与参考 PG 成功/自身错误分开。

RETURNING 只返回实际插入行，跳过行零返回、影响行数 0；混合批次只计成功插入行。无 RETURNING 无结果集，有 RETURNING 即使零行也保持固定列描述，不能返回冲突旧行。成功不使事务进入 E；真实错误遵守本章失败事务与 Sync 边界。所谓“不写入”仅指跳过候选不产生目标表插入/覆盖，不承诺不求值、不加锁或所有副作用为零：sequence 的 nextval 可在冲突前消耗且不回滚。Read Committed 下并发提交可能使当前快照不可见的冲突行阻止插入；不能由快照 SELECT 无行推出必须插入。

官方依据均于 **2026-10-23** 查阅，适用 **PostgreSQL 18**：[INSERT / ON CONFLICT / Outputs](https://www.postgresql.org/docs/18/sql-insert.html) 支持目标推断、仲裁限制、跳过及计数/返回规则；[约束 §5.5](https://www.postgresql.org/docs/18/ddl-constraints.html) 支持独立约束错误边界；[隔离 §13.2.1](https://www.postgresql.org/docs/18/transaction-iso.html) 支持并发快照边界；[序列 §9.17](https://www.postgresql.org/docs/18/functions-sequence.html) 支持 nextval 例外；[事务 §3.4](https://www.postgresql.org/docs/18/tutorial-transactions.html) 支持原子性/恢复；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 仅确定错误类别名称，不证明某条 SQL 的实测诊断。在线附录当日自述 18.6 错误码表，但没有本地 18.6 执行或版本固定差分证据，不升级为该补丁版本行为通过。

#### 冲突更新执行契约

复用前块仲裁推断/约束命名契约，但 DO UPDATE 必须指定目标，仅开放可用的非延迟唯一仲裁；排斥约束不能作为该动作仲裁。合法但尚未开放形式要求写前能力拒绝（产品目标 0A000，当前阶段/诊断未验证），不能删除子句变普通 INSERT、改成 REPLACE 或 MySQL ON DUPLICATE KEY UPDATE。

| 边界 | 目标与可观察区分 |
| --- | --- |
| 两种行身份 | SET/动作 WHERE 的 dst 是被仲裁命中的已有行，excluded 是拟插入行，含已求值默认值；PG 的 BEFORE INSERT 行触发器修改也属于 excluded，触发器能力另过门槛。别名隐藏原表名，SET 左侧写列名，不能写 dst.v；不把 excluded 绑定为持久表或更新后的行。 |
| 更新表达式 | 用旧行与候选行计算新值，未列入 SET 的旧列保留；无冲突时直接插入候选，不能执行更新表达式。类型/名称/权限沿第 2–4、6 章，INSERT/UPDATE/被读取列 SELECT 权限须独立满足。 |
| 两类 WHERE | 目标列表后的 WHERE 用于仲裁索引推断；DO UPDATE SET 后的 WHERE 是找到并锁定候选冲突行后判断的 bool 条件。只有 true 才更新，false/SQL NULL 均跳过；不能合并成插入过滤，也不能因 false 自动改走插入。 |
| 返回与计数 | RETURNING 默认输出实际插入/更新后的行；条件跳过零返回但保持固定列描述。计数为插入加更新的行数，即便更新值未变也计该更新行；跳过不计。命令标签仍为 INSERT 0 n。无 RETURNING 无结果集。沿前块 OLD/NEW 的独立门槛，不额外承诺。 |
| 错误与重复命中 | 非仲裁唯一错误、更新产生的二次唯一/CHECK/NOT NULL 等错误不能被冲突动作吞掉；整条语句失败不能保留前面插入/更新。一个语句不能重复影响同一目标行，应以 cardinality violation 拒绝，不采用最后一行覆盖；候选之间冲突及 WHERE 跳过组合另采边界诊断，不能泛化为所有重复输入都同样失败。 |

并发目标来自 PG Read Committed：无独立错误且无条件跳过时，候选应原子地插入或更新；冲突行即便不在语句常规快照中可见也可能被更新。动作 WHERE 不成立仍可能持锁；高隔离级别可能失败，不能承诺所有并发都成功或把共享 TSO/悲观锁等同 PG 隔离语义。必须补真实仲裁键竞争、等待/提交/回滚、条件重判及最终唯一行证据，禁止 SELECT 后 INSERT 的竞态模拟。共享执行链能否满足仍待独立取证。

官方依据于 **2026-10-23** 访问，适用 **PostgreSQL 18**：[INSERT / ON CONFLICT / Outputs](https://www.postgresql.org/docs/18/sql-insert.html) 定义行身份、动作条件、仲裁限制、确定性与返回计数；[隔离 §13.2](https://www.postgresql.org/docs/18/transaction-iso.html) 定义 Read Committed 并发可见性与较高隔离的重试边界。事务失败与 Sync 沿本章公共契约。MySQL 独立对照依据 **8.4** [ON DUPLICATE KEY UPDATE](https://dev.mysql.com/doc/refman/8.4/en/insert-on-duplicate.html)：没有 PG 指定仲裁/动作 WHERE 的等价承诺，插入/更新/未改变值的 affected rows 口径不同，须记录 CLIENT_FOUND_ROWS。以上不是精确 **18.6** 参考构建或运行证据；本轮未核实该补丁行为与精确错误字段。

#### 只读公共表表达式契约

本块开放目标为**非递归、无写操作及无 volatile 调用的 SELECT CTE + 主 SELECT**；普通表达式、连接与排序沿第 2–4 章能力边界。CTE 是单条语句的命名行集，不创建持久对象或跨语句缓存。成功返回完整行、重复项与 SQL NULL，只有最终 ORDER BY 保证输出顺序。外层过滤不能丢失或串扰其他引用的行。

未限定名优先绑定可见 CTE，显式 public 表名绕过 CTE；内层同名遮蔽退出后恢复。非 RECURSIVE 定义体可见先前同级与合法外层名称，不能提前绑定未来定义；自身同名引用仍可能绑定外层 CTE 或真实表。此处沿第 2 章规则检验**结果**，不把名字相同一概判作递归。重复定义、缺列、不可见关系应在适当分析阶段拒绝，不返回部分成功结果；精确诊断字段依参考端采集。

输出列按定义体的定型结果继承，显式列名列表按位置覆盖前若干列名，未列名的尾列保留原名；列表比实际列多须失败。列重名本身不能一概当非法，引用产生歧义时另验。外层范围别名不能改变列类型，零行和全 NULL 也保留固定身份；Describe 不运行查询取样。PG OID/typlen/typmod、可空性和来源表/列字段依第 3 章固定元数据契约逐项采集，CTE 不分配伪造持久 OID。

重复引用须产生同一语句语义下的行集，不能把第一个消费者的游标耗尽状态交给第二个。这里不承诺所有 SELECT 都无副作用：volatile 函数、sequence、行锁、错误表达式与未使用 CTE 的求值时点需要独立门槛；纯值正例不能证明调用次数正确。默认可折叠单引用、默认多引用共享求值、显式 MATERIALIZED / NOT MATERIALIZED 的优化与求值约束分别取证，不能仅凭结果相等宣称物化契约完成。未开放时明确能力拒绝（产品目标 0A000，当前入口阶段/诊断未验证），不得静默去掉关键字。

RECURSIVE、无环前向引用、工作表终止/去重/资源释放、SEARCH/CYCLE 单独验收；现有 PG 入口递归拒绝保留，不由共享 Rust 递归入口存在推导支持。普通 SELECT CTE 的合法性不能授权写入 CTE 或主 DML。

官方依据（**2026-10-23** 访问，适用 **PostgreSQL 18 大版本**）：[§7.8 WITH](https://www.postgresql.org/docs/18/queries-with.html) 的 SELECT、递归、物化分节定义行集与求值边界；[SELECT / WITH 与 FROM](https://www.postgresql.org/docs/18/sql-select.html) 定义定义顺序、列名及遮蔽；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)核对下方错误名对应码，不单独证明具体输入的诊断。官方 **REL_18_0** [parse_cte.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/parser/parse_cte.c) 的 transformWithClause / analyzeCTETargetList 补证同层重复名与部分列别名规则；这仍不是 18.6 运行证据。**18.6** 仍是来源指定目标，未建立该补丁参考实例或差分结果。下列结果为按规则推导的验收目标，非运行报告。

#### 写入公共表表达式契约

窄开放目标是顶层 `WITH moved AS (DELETE ... RETURNING ...) INSERT ... SELECT ... FROM moved RETURNING ...` 的普通表搬移：CTE 行集来自同一次 DELETE 的被删行投影，外层 INSERT 写入该行集，客户端只接收主语句的返回行。默认值、NULL、别名、列身份和零行描述复用返回契约；写后行必须再查询两表证明，不能把 DELETE 行计入主 INSERT 影响行数，不能靠重扫源表重建 moved。无外层 RETURNING 时不产生成功行集，仍须验证真实副作用。

同语句的主语句及各 CTE 使用同一快照，兄弟写入的实际先后不构成承诺；普通表扫描看不到本语句其他写入分支的变化，变化只能由 RETURNING 行集传递。即使执行器采用顺序调度，也不能让后一分支直接读到前一分支的新表状态。写入 CTE 每次语句执行一次且执行到完成，不因未引用、零消费或多次引用而跳过、截断或重做；这与只读 CTE 的按需规则不同。主 SELECT/无 RETURNING 的合法组合可作为后续独立门槛，未开放时明确拒绝，不能当成 PG 非法形式。

搬移必须作为一条原子语句接入共享事务：DELETE 或 INSERT 任一侧执行失败、表达式失败或取消，均不能留下部分持久表写入。隐式事务失败后两表保持原状；显式事务错误进入 E，Sync 不恢复事务，ROLLBACK 后两表及此前事务写入均恢复。成功返回值也不等于提交，COMMIT/ROLLBACK 和观察连接的证据分别采集。sequence、外部函数等非事务副作用不在“两表回滚”承诺内。

写入 CTE 仅允许附着顶层语句；直接递归自引用的数据修改在 PG 本身不允许。递归只读 CTE 提供键集再驱动 DML 是另一种合法形式，不能与递归写入混称，当前仍保留 RECURSIVE 门禁。主语句/兄弟分支重复修改同一行不提供确定顺序或结果承诺，PG 也有不可预测边界，不为其编造“后写覆盖”目标。共享内核不能满足本块行集、快照或原子性时，整体在写入前拒绝（产品限制目标 0A000，当前阶段/诊断未测）；禁止丢弃 WITH、RETURNING 或只执行剩余 INSERT。

官方依据（**2026-10-23** 访问，适用 **PostgreSQL 18 大版本**）：[§7.8.4](https://www.postgresql.org/docs/18/queries-with.html#QUERIES-WITH-MODIFYING)定义顶层附着、RETURNING 通道、一次完成、同快照、递归及重复修改限制；[DELETE](https://www.postgresql.org/docs/18/sql-delete.html)和 [INSERT](https://www.postgresql.org/docs/18/sql-insert.html)定义实际删除/插入行及返回投影；[SELECT / WITH](https://www.postgresql.org/docs/18/sql-select.html)补充 WITH 组合边界；[§3.4](https://www.postgresql.org/docs/18/tutorial-transactions.html)支持单语句隐式事务及整体回滚；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)核对下方错误类别，不能单独证明具体 SQL 的诊断。虽然附录当前写有 18.6，仍未核验精确补丁发布、建立 18.6 实例或运行差分；下方均为按规范推导的目标。

#### 模式与搜索路径契约

原始设置、有效有序路径和实际绑定对象必须分开：SHOW 返回设置文本；有效路径按存在性、USAGE、用户项等解析；未限定关系选有效路径中第一个匹配对象，显式 schema.t 独立检查存在性/权限，不因路径排除而不可见，也不能失败后回落其他库。current_schema 取首个有效显式 schema，无则 SQL NULL。目录展示不能替代真实对象读取、写入或权限证明。

| 状态/操作 | PG18 目标与能力边界 |
| --- | --- |
| SET/SHOW/RESET | SET 及 SET SESSION 只影响当前连接；TO 与 = 等价。RESET/SET TO DEFAULT 恢复“本会话未发 SET 时”的配置值，不固定写死 public；隔离验收夹具将参考端默认显式配置为 public，同时另采原生默认差异（PG 通常为 `"$user", public`）。现有默认 public 是限制，不将其写成 PG 默认。 |
| 空路径与顺序 | `SET search_path TO ''` 保留空原值，public 对象不再由未限定名可见；省略 pg_catalog 时隐式优先，显式列出则按位置搜索。空显式路径无默认用户建表位置，不能回落当前库；此处不授权事务型 DDL。 |
| 缺失/用户 schema | 完整目标忽略不存在或无 USAGE 的路径项，不能把当前 missing→0A000 写成 PG 要求；`$user`、临时 schema 及任意用户 schema 需要工作流 6 的真实身份与权限后才开放，当前不承诺。关系/类型的临时项规则不直接套给函数/运算符。 |
| 事务成功 | I 下 SET 形成会话值；BEGIN 内普通 SET/RESET 在 T 中立即可见，COMMIT 后保留；另一连接始终不受影响。 |
| 事务失败与回滚 | BEGIN 前值须可恢复；BEGIN 内 SET/RESET 后 ROLLBACK 恢复事务前值。出错进入 E 后 SHOW/SET/RESET 不能绕过失败门禁或清 E；扩展 Sync 仅同步协议，ROLLBACK 后恢复旧路径并进入 I。精确诊断及错误发生阶段留给实际采证。 |
| SET LOCAL | 目标仅在当前事务有效，COMMIT/ROLLBACK 均撤销 LOCAL；同事务先 SET 后 LOCAL，提交后恢复该 SET 值，回滚则回到事务前值。事务外 LOCAL 的 PG 目标是警告且无效果。当前 SessionQuery 没有 LOCAL/SESSION 分支，不能由返回 None 推断全入口的精确拒绝；不满足作用域恢复前须在副作用前明确门禁，拒绝诊断待采，不能当普通永久 SET 接受。 |

依据为 PostgreSQL **18 主版本**官方 [search_path 参数](https://www.postgresql.org/docs/18/runtime-config-client.html#GUC-SEARCH-PATH)（原始/有效路径、权限与隐式目录）、[Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html#DDL-SCHEMAS-PATH)（对象选择）、[SET](https://www.postgresql.org/docs/18/sql-set.html)（连接与事务作用域）、[RESET](https://www.postgresql.org/docs/18/sql-reset.html)（配置默认及回滚），访问日期均为 **2026-10-23**。它们支持目标规则，不能证明 AsterSQL 或计划指定 18.6 的精确运行诊断；本轮未核实 18.6 发行/运行行为。

#### 引号与大小写执行契约

PG 未引号 t、T 与引用 "t" 选择同一小写名称；"T" 为精确大写名称。该身份必须从第 2 章解析保真传到关系/列/别名绑定、真实目录与共享执行，不得在后端统一 lower 后误命中。public.t、PUBLIC.T、"public"."t" 同指当前连接库中 public 的 t，public."T" 只指精确大写对象；"PUBLIC" 与 public 不等价，现有限 public 门禁须显式保留。引号内点号是名称字符：public."a.b" 是一个关系名，不是 a schema 的 b；单引号字符串中的 public.T 不触发解析对象或路径变更。表别名建立后隐藏原表引用，未引用别名也折叠，引用别名精确匹配；输出别名还须在结果列名中保真，不用相同值掩盖列名差异。

共享内核及目录的大小写规则、lowercase 键、表名配置可能无法表示同 schema 的 t/"T" 或同表 c/"C"。在真实存储、目录与执行身份贯通前，应在创建/绑定或写入前明确能力拒绝，记录限制及实际阶段，不能静默覆盖、合并、回落或读写另一对象。产品门禁目标 0A000，不代表 PostgreSQL 对该合法输入也拒绝；本轮不选命名编码或改造方案。MySQL 连接继续按自身 sql_mode、名称类别、lower_case_table_names 与平台行为执行，禁止为 PG 修改全局设置、共享折叠键或 MySQL 模式。

两段 public 限定仍受当前连接库隔离；三段当前库.public.t 与跨库须区分：PG18 允许写当前数据库前缀，拒绝其他数据库。当前适配也仅放行当前库三段形式，不能把任意 MySQL 库限定当 PG schema。精确诊断待参考采集，不从内部字符串推断 SQLSTATE/位置。

官方依据（访问 **2026-10-23**，适用 **PostgreSQL 18 主版本**）：[§4.1.1 标识符](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-SYNTAX-IDENTIFIERS)支持折叠、引号和特殊字符；[§5.10.1 schema 限定](https://www.postgresql.org/docs/18/ddl-schemas.html#DDL-SCHEMAS-CREATE)支持当前库前缀及跨库限制；[§7.2.1.2 表与列别名](https://www.postgresql.org/docs/18/queries-table-expressions.html#QUERIES-TABLE-ALIASES)支持别名隐藏原名。验收值为这些规则的文档推导，不是运行结果；18.6 仍是来源目标，未核实精确补丁行为。MySQL 自身比较按 [8.4 名称大小写](https://dev.mysql.com/doc/refman/8.4/en/identifier-case-sensitivity.html)及第 2 章模式边界，不能从 PG 规则推断其结果。

#### 预处理执行时会话契约

以下为目标，观察对象身份、参数身份、列身份和运行时值分别记录，不能笼统称为“执行时绑定”。

| 阶段 | 名称环境、参数与结果身份观察边界 |
| --- | --- |
| Parse | 对当时名称环境做语法分析、语义分析/重写，选择关系、列、函数及类型；不是只保存未分析文本。参数编号和声明/推断类型确定，参数值尚无；结果类型/列形状来自分析，不能取首行或执行写入来 Describe。当前普通路径需显式 OID 的限制仍单列。 |
| Bind | 按该 statement 的参数身份解码本次值并选择结果格式，创建本连接 portal；PG 通常在此规划，需复核依赖失效及两次使用之间的 search_path 变化并按新环境重新分析。值变化不能改变参数类型或随意改变已准备结果类型；合法重绑定到另一对象也受固定结果类型约束。 |
| Describe | statement 返回参数及结果描述（结果格式为 0），portal 返回该次绑定的列/格式；statement 的依赖重验证可能报错，不以“只 Describe”绕开失效检查。不能执行原语句采样；表 OID/属性号、列名与类型/typmod分别记录，固定类型不等于把物理对象 OID永久冻结。 |
| Execute | 执行已绑定 portal 并求运行时值，如 current_schema()；不是每次把原 SQL 按最新路径重解析。已绑定 portal 与后续新 Bind 的语句使用须区分，暂停/续取不得重查目录或重复副作用。取数快照与名称/类型定型是两件事，跨连接 DDL 不承诺无视锁立即生效。 |

两次使用间 search_path 改变、被依赖对象定义改变应触发相应重新分析/规划；删除后缺对象应失败，删除重建且输出兼容可绑定新对象，输出类型/列形状改变应拒绝并由客户端 Close/重新 Parse 获取新描述。固定参数类型不得静默随 DDL 改变。路径文本不变而更早 schema 新建同名表是特例：仅该动作不自动使原依赖失效，不保证马上切换，须有后续强制重新分析条件；显式限定名不因路径切换转向别的 schema。会话函数的当前值变化不能作为所有关系/函数都延迟绑定的证明。

官方依据（2026-10-23 查阅，适用 PostgreSQL **18**）：[PREPARE](https://www.postgresql.org/docs/18/sql-prepare.html) 说明分析/重写、会话归属、DDL/路径变化及更早同名表特例；[§54.2.3 Extended Query](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-EXT-QUERY) 说明 Parse/Bind/Describe/Execute、参数身份、通常 Bind 规划及 portal 生命周期。固定结果类型另核官方 PG18 维护分支 [postgres.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/tcop/postgres.c) 的 exec_parse_message/CompleteCachedPlan(fixed_result=true) 和 [plancache.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/utils/cache/plancache.c) 的 RevalidateCachedQuery（参数类型保持、结果不兼容时 FEATURE_NOT_SUPPORTED）。该分支是源码依据，未固定为 18.6，也无精确补丁或参考端运行证据；精确报文、SQLSTATE、错误阶段与位置在下列用例采集，不能拿产品元数据不一致报错代替 PG 固定类型诊断。

#### 序列执行契约

以下为目标，真实序列与 AUTO_INCREMENT 分开登记。`nextval(regclass) → bigint` 原子分配；`currval(regclass) → bigint` 读本连接该序列最近值，未初始化应错误；`setval(regclass,bigint) → bigint` 与三参 true 形式都回报设定值并更新本连接 currval，随后 nextval 按步长推进；三参 false 使下一次 nextval 恰返回设定值，不改变本连接 currval（也不初始化它）。各连接私有最后值按真实对象身份隔离，其他连接 nextval/setval 不替换它；断开重连不继承。PG 无参 lastval 与原生带名称 last_val 也不能混同，本块不由此开放 lastval。

持久序列水位/缓存分配与连接最后值是两种状态：成功 nextval 消耗和 setval 变更不随用户 ROLLBACK 撤销，用户表写入仍须回滚；setval 的共享状态对其他事务可见，但 CACHE>1 的已有预分配值可能继续使用。无重置、无 CYCLE 的并发分配不得重复；跨连接次序、间隙、提交次序均不能当唯一性证明，setval 回设和 CYCLE 允许重用历史数值。正常进程内跨事务可见性不证明崩溃持久性；未提交分配遇崩溃后的状态不可据此承诺，缓存水位也不是本连接 currval。

权限目标分别为 nextval 的 USAGE 或 UPDATE、currval 的 USAGE 或 SELECT、setval 的 UPDATE。缺对象、对象非序列、无权限、首次 currval、越界及 NO CYCLE 耗尽须失败，不伪造值或退回 AUTO_INCREMENT。失败后的连接/事务门禁沿本章 I/T/E 与 Sync 契约；未开放的合法调用保留能力拒绝，诊断阶段与精确 SQLSTATE 待参考采集，不沿用原生 NULL 或权限映射。

官方依据（2026-10-23 查阅，适用 PostgreSQL **18**）：[§9.17 序列函数](https://www.postgresql.org/docs/18/functions-sequence.html) 支持上述签名、权限、连接状态与非回滚边界；[CREATE SEQUENCE](https://www.postgresql.org/docs/18/sql-createsequence.html) 支持对象、CACHE/CYCLE、边界和缓存观察规则。它们是大版本规范，未取得精确 18.6 实例、补丁固定证据或差分结果，不能提升为 18.6 实测支持。下方数值是限定夹具下按规则推导的目标。

#### 标识列生成契约

目标为普通单表 INSERT 的真实 identity 生成；列值类型由声明的 smallint/integer/bigint 决定，不能因序列取值容器为 bigint 就统一返回 bigint。省略 identity 列或显式 `DEFAULT` 都请求所关联序列的新值；`NULL` 不是 DEFAULT。identity 隐含 NOT NULL，不隐含 UNIQUE，唯一性须单独声明约束。以下策略只在声明类型/范围及约束通过时产生行。

| 输入策略 | GENERATED ALWAYS | GENERATED BY DEFAULT |
| --- | --- | --- |
| 无覆盖子句，显式非 DEFAULT 值 | 拒绝；不能默默生成或接受 | 使用显式值 |
| OVERRIDING SYSTEM VALUE | 接受显式值；DEFAULT/省略仍生成 | 接受显式值，此覆盖子句合法且不改变本来策略 |
| OVERRIDING USER VALUE | 忽略供应给 identity 的值，改用序列生成 | 同样改用序列生成 |

接受显式值不替序列自动 rebase；以后生成值仍可能与人工值冲突。不得套用 MySQL 显式高值推动 AUTO_INCREMENT 水位或 NULL 请求生成的规则。`INSERT ... DEFAULT VALUES` 请求各列默认值，但该形式不能带 OVERRIDING。首批若未具备某一合法 PG 形式，应在写入/分配前明确拒绝并列产品限制，不能删掉覆盖子句、转换成 AUTO_INCREMENT 或把合法形式称为 PG 语法错误；该边界是目标门槛，未声称所有当前解析分支已满足。

生成仅在执行写入时发生；Parse/Bind/Describe 不分配，RETURNING 返回实际插入行的最终 id 和其他列。成功分配后失败或用户 ROLLBACK 不回收序列值，表行须保持语句/事务原子性，复用[序列执行契约](#序列执行契约)的连接状态、非回滚和缓存边界。分析时拒绝显式值与运行期约束失败分开，不对所有失败笼统承诺“不消耗”。Sync 只恢复扩展消息流，显式失败事务仍须 ROLLBACK；序列水位与用户行回滚分别验收。

工作流 6 必须提供真实列的 identity 模式、类型/NOT NULL、稳定表/列 ID、所关联序列 ID 和参数、列拥有依赖及生命周期；pg_attribute.attidentity 为 a/d，普通列为空，不能从名字、默认 SQL 或 AUTO_INCREMENT 猜测。identity 内建序列到表列为 internal（i）依赖，与 serial 常见 auto（a）依赖区分；pg_get_serial_sequence 应按真实关联定位序列，不硬编码自动名称。默认表达式与 identity 属性独立：普通 DEFAULT/serial 的 pg_attrdef、atthasdef、pg_get_expr 必须来自实际表达式及解析后的对象引用，不能为 identity 强造 nextval 文本；本块无其他默认声明的identity夹具应无对应pg_attrdef行且atthasdef=false；此为官方源码推导目标，仍须参考目录实测，不能用 DEFAULT 关键字推断存在普通默认表达式。内省一致只是必要条件，不能替代写入和回滚证明；当前缺少的拥有关系交工作流 6，阻止开放相关能力。

官方依据均于 **2026-10-23** 查阅，适用 **PostgreSQL 18**：[§5.3 Identity Columns](https://www.postgresql.org/docs/18/ddl-identity-columns.html) 定义隐式序列、模式及约束；[INSERT](https://www.postgresql.org/docs/18/sql-insert.html) 定义两种覆盖、DEFAULT VALUES 与返回行；[§9.17](https://www.postgresql.org/docs/18/functions-sequence.html) 定义分配非回滚。[pg_attribute](https://www.postgresql.org/docs/18/catalog-pg-attribute.html)、[pg_attrdef](https://www.postgresql.org/docs/18/catalog-pg-attrdef.html)、[pg_depend](https://www.postgresql.org/docs/18/catalog-pg-depend.html) 和 [pg_get_serial_sequence](https://www.postgresql.org/docs/18/functions-info.html) 支持目录身份和表达式边界。i/a 另核官方 PG18 维护分支 [sequence.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/commands/sequence.c)::process_owned_by 的 for_identity 分流与表列引用；[parse_utilcmd.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/parser/parse_utilcmd.c)::transformColumnDefinition 把identity属性与raw_default分开并拒绝同时声明，[rewriteHandler.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/rewrite/rewriteHandler.c)::build_column_default 为identity构造带真实seqid/typeId的NextValueExpr，区别于atthasdef普通默认表达式路径，据此推导本夹具的默认目录目标。该分支和大版本手册未固定为 **18.6**；未核实精确补丁发布或采集参考实例，以下数值均为限定夹具推导目标。

#### 事务提交与回滚契约

BEGIN 后的正常 DML 属于同一所属连接的共享事务：A 能读取自己的写入，B 的独立新查询不能读取 A 未提交写入；COMMIT 成功结束后，B 的新快照能读取提交值；ROLLBACK 撤销该事务全部普通表行写入，恢复原值与删除行。不能只撤销最后一条语句，也不能把返回标签、影响行数或 T→I 当作副作用证明。两条最小序列和每步 I/T/E 见下方验收。B 始终 autocommit、每次查询取得新快照，避免把 B 已开启的旧快照误报为提交不可见；不据此推广为全部并发异常/隔离级别兼容。

| 边界 | PG18 目标及本地能力门槛 |
| --- | --- |
| 隐式事务 | 无 BEGIN 的单条简单 Query 成功自动提交，失败则撤销该条写入并返回 I。扩展协议的隐式事务通常到 Sync 成功提交/失败回滚；同一简单 Query 的多语句隐式块不能拆成各自自动提交。本块基本用例逐条 Q 或每条 Execute 后 Sync；多语句 Q、无 Sync 的多次 Execute 原子性留后续协议专项，当前不可从 shared in_transaction 或单语句通过推定支持。 |
| 重复 BEGIN | 已在正常事务内再 BEGIN，PG 发出警告且保持原事务，不能隐式提交原写入或创建嵌套事务。本地未验证，若共享引擎的再次 BEGIN 会结束旧事务，须在 PG 边界拒绝或适配，不能发布等价通过；精确拒绝策略待独立实施取证。 |
| 事务外结束 | I 下普通 COMMIT/ROLLBACK 的 PG 目标为警告且无行副作用，随后仍 I；警告 SQLSTATE、S/V/M 与标签待参考采集，不猜诊断。AND CHAIN 不属于本块。 |
| 断连 | 明确未发 COMMIT 的显式事务在正常 Terminate 或异常连接关闭被服务端识别后须回滚；B 以有界等待并确认会话清理完成后观察原值，不用任意 sleep 冒充清理屏障。若断连与 COMMIT/隐式提交执行竞态，客户端可能不知道结果，不能保证回滚；须另取提交结果和重试语义证据。 |
| 失败后的出口 | 失败态不得继续提交部分写入；当前产品要求显式 ROLLBACK，Sync 仅同步流水线且显式事务仍 E。PG 参考端失败后 COMMIT 的实际响应、标签及数据结果另采，当前 25P02 策略不得写成参考端一致。后续批次细化失败态与协议恢复。 |

状态分层遵循本章公共模板：共享 in_transaction 描述底层事务，PgSession.failed_transaction 描述 PG 失败语义，Extended.failed 描述协议丢弃流水线，三者不能互换；BEGIN 后即使尚未分配存储事务/TS，也应按显式 SQL 事务契约报告 T。TSO/StartTS 不等于 PG XID，不能拿 txid_current 或 tidb_trx 查询代替行可见性证明。仅为测试观察选择参考 PG 的 READ COMMITTED；本地须记录真实隔离配置，若不能设置或证明所需边界则登记能力缺口，禁止宣称未验证的 PG 默认隔离、RR、Serializable 或锁冲突语义。

官方依据于 **2026-10-23** 访问，均适用 **PostgreSQL 18 大版本**：[§3.4 Transactions](https://www.postgresql.org/docs/18/tutorial-transactions.html)用于普通行写入的原子性与回滚；[BEGIN](https://www.postgresql.org/docs/18/sql-begin.html)用于自动提交、重复 BEGIN；[COMMIT](https://www.postgresql.org/docs/18/sql-commit.html)与[ROLLBACK](https://www.postgresql.org/docs/18/sql-rollback.html)用于结束事务和事务外警告；[§13.2 Transaction Isolation](https://www.postgresql.org/docs/18/transaction-iso.html)用于新快照观察与隔离限制；[§54.2 Message Flow](https://www.postgresql.org/docs/18/protocol-flow.html)用于隐式块/Sync 边界及 §54.2.9 断连清理和提交竞态；[§54.7 ReadyForQuery](https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-READYFORQUERY)用于 I/T/E。来源指定 **18.6** 的精确补丁发布/行为未核实，无该版本实测；提交后崩溃持久性是后续存储验收条件，本轮无运行结论。

#### 失败事务恢复契约

目标为普通行 DML 的显式事务失败后禁止继续读写及误提交，完整 ROLLBACK 恢复连接可用性。I/T/E 沿公共模板；共享事务、PG 失败标志和扩展流水线错误分别记录。以下表在未使用保存点、未断连、每条 SQL 独立发送且消息合法的条件下定义目标，实际响应另存，不以源码推测填充。

| 输入与前态 | PG18 目标状态与副作用 | 当前产品限制/待证差异 |
| --- | --- | --- |
| I → BEGIN → 写入 | BEGIN 后 T；成功行写入留在同一事务，其他连接看不到未提交值。 | 沿前块验证共享事务，不能只看 T 或命令标签。 |
| T → 唯一键错误 / SELECT 1/0 | 分别目标 23505 / 22012，进入 E；普通事务行写入不能成为已提交结果。 | record_error 依赖错误后 in_transaction，23505 有独立测试意图，22012 映射与错误阶段未实测；引擎返回 NULL/警告或 XX000 均不能算 PG 目标通过。 |
| E → 普通 SELECT、INSERT、UPDATE、DELETE | 拒绝（25P02），仍 E，无成功返回行或新写入；会话设置和目录读也不能成为绕过事务门禁的入口。 | Q/Parse 有检查；已准备 Execute 缺重新检查的静态风险须用真实行值取证，不预设底层会补拦截。语法/报文错误的优先级不在本行硬定为 25P02。 |
| E → COMMIT（无 CHAIN） | PG18 参考目标结束失败事务并回滚，回 I，完成标签为 ROLLBACK，错误前的普通行写入不提交。 | 当前门禁返回 25P02、仍 E，须再 ROLLBACK；这是产品限制，不能写成 PG 等价，也不能只把标签改为 ROLLBACK。 |
| E → 完整 ROLLBACK（无 CHAIN） | 撤销整段事务行修改，回 I；随后新查询和新事务成功，失败标志不传播到其他/新连接。 | record_success 清标志只是局部接线，真实撤销及跨连接值需取证。回滚执行失败不得伪报 I 或成功恢复，须记录底层状态并按连接错误策略处理。 |
| E → Sync；E → 已有保存点的 ROLLBACK TO | Sync 只恢复扩展消息处理，显式事务仍 E。PG 的 ROLLBACK TO 可撤销保存点之后修改并回 T，保留更早修改。 | 本块仅交接 Sync 区分，完整协议矩阵归批次14；保存点/子事务、命令分类与局部清失败必须独立实现验收，不从“ROLLBACK 可放行”推断 ROLLBACK TO 支持或允许其结束整段事务。 |

这里的“撤销”限定普通事务表行，不撤销前块定义的序列/identity 消耗；也不把事务外单语句错误一律变成持续 E。错误原始 SQLSTATE、后续 25P02 与恢复命令结果分开存储，首个失败不能被后续拒绝覆盖。

官方依据于 **2026-10-23** 查阅，适用 **PostgreSQL 18 大版本**：[§3.4 Transactions](https://www.postgresql.org/docs/18/tutorial-transactions.html)说明失败块与保存点恢复；[ROLLBACK](https://www.postgresql.org/docs/18/sql-rollback.html)定义完整撤销；[ROLLBACK TO SAVEPOINT](https://www.postgresql.org/docs/18/sql-rollback-to.html)定义局部撤销及保存点保留；[附录A](https://www.postgresql.org/docs/18/errcodes-appendix.html)对应 23505、22012、25P02；[§54.2 Message Flow](https://www.postgresql.org/docs/18/protocol-flow.html)区分显式事务和 Sync。COMMIT 在失败块中实际走回滚的目标补充取自 PostgreSQL 官方 **REL_18_STABLE** 源码 [xact.c::EndTransactionBlock](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/access/transam/xact.c) 的 TBLOCK_ABORT 分支及 false 返回、[utility.c::standard_ProcessUtility](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/tcop/utility.c) 的 CMDTAG_ROLLBACK 分支（此次读取同仓库 raw 内容）；这是按官方源码作出的静态目标判断，不是参考端运行结果或固定补丁源码。来源指定 **18.6** 的精确运行行为、消息字段与补丁发布状态本块未独立核实，不改前章基线声明。

#### 协议同步与事务恢复边界

分别记录协议“正常/丢弃至Sync”与SQL事务I/T/E，不用一个失败位表达二者。下列为普通表DML目标；Sync不是SQL回滚命令，但无BEGIN的扩展隐式事务须在同步边界取得提交/撤销证据。

| 初态与序列 | 协议状态与SQL事务目标 |
| --- | --- |
| 显式事务内：BEGIN→成功写入→扩展Execute错误→后续消息→Sync→ReadyForQuery | BEGIN后T；首错使SQL进入E并开始丢弃。至Sync前的后续Parse/Bind/Execute及Q均不执行，无对应成功响应/副作用。Sync退出丢弃、返回ReadyForQuery(E)，不得提交早先写入或伪报T/I。 |
| 显式事务外：I→扩展成功写入→同一未同步段Execute错误→后续消息→Sync→ReadyForQuery | 错误后丢弃至Sync；普通DML隐式段的成功前缀也须撤销，Sync后ReadyForQuery(I)，下一独立查询可执行，无需额外ROLLBACK。不能将每次Execute先自动提交再返回I称为通过。 |
| 成功对照：无BEGIN的一段成功DML→Sync；显式T中成功DML→Sync | 前者在边界成功提交并I；后者保持T、对其他连接不可见，直到实际COMMIT。不能从CommandComplete推定已提交。 |

显式E在Sync之后仍拒绝普通读写，Q与扩展Execute共享门禁；首错前绑定的portal也不能成为绕过失败状态的入口。完整ROLLBACK成功且真实撤销后才回I并可新建语句继续；失败态COMMIT的PG回滚目标与当前25P02产品限制沿用前块，保存点局部恢复不在本题展开。客户端应先Sync取回同步边界，再发恢复SQL；丢弃期间发送的ROLLBACK本身不会执行，Flush也不结束丢弃。只对可恢复扩展错误作此承诺，失去消息边界、连接关闭或致命错误交工作流7。

Parse语法/分析错误、Bind参数转换错误、Execute执行错误分开记首错阶段；三者均需独立采事务内E、事务外I及前缀写入是否撤销，不能拿“Parse没有执行DML”推导整个事务无影响。取消属于正在执行命令的失败：只在已证明命中活动执行、原连接确实收到取消错误后进入相应恢复断言；发出CancelRequest或取消连接关闭不等于取消成功，空闲/过期取消不能污染下一命令。错误字段、取消身份/竞态、portal销毁及prepared存活规则交工作流7，本章只要求其不能泄漏行写入或绕过事务门禁，不发明新接口。

官方依据于 **2026-10-23** 访问，适用 **PostgreSQL 18大版本**：[§54.2.3扩展查询及§54.2.4流水线](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-EXT-QUERY)用于丢弃至Sync、隐式事务边界和显式块保留；同页[§54.2.8取消](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-CANCELING-REQUESTS)用于取消结果须从原连接观察；[§54.7 ReadyForQuery](https://www.postgresql.org/docs/18/protocol-message-formats.html#PROTOCOL-MESSAGE-FORMATS-READYFORQUERY)用于I/T/E；[附录A错误码](https://www.postgresql.org/docs/18/errcodes-appendix.html)核对23505、25P02、42601、22P02、57014的条件名称（页面表格标注18.6，仅作代码名称依据）。以上是规范目标，未运行PG参考端；来源指定18.6的补丁发布、精确诊断和运行行为未核实。

#### 事务型建表能力门槛

事务型建表先做独立、局部可行性验证，目标限于普通持久表 `CREATE TABLE → INSERT → ROLLBACK`。同连接应读到自己未提交的表与行；另一连接不得看到未提交对象/行；回滚后双方目录、名称解析和数据都无该表，已有表的先行写入也撤销，以检测隐式提交。提交分支须使对象、列/主键及数据在同一用户事务边界对外可见。只看到 CREATE TABLE 标签、内省伪行或事后补偿 DROP，均不证明原子性。

| 方案 | 权衡与决定 |
| --- | --- |
| 在 PG 边界保留显式事务 DDL 拒绝 | 阻止进入已知不适配的事务路径，能力有限；仍须证明拒绝发生在副作用前、不提交前缀写入，并覆盖简单/扩展入口。当前保留。 |
| 在共享会话/DDL/存储做有界原子性原型 | 可验证目标，但依赖同事务元数据可见性、schema version/目录失效、锁与数据提交/回滚协调。先拿最小探针证据，确认底层能提供，再另立实施计划；本轮只写规格。 |
| 改 CommandComplete 或回滚后补偿删除 | 标签不改变提交边界，补偿无法撤回其他连接已见状态，且可在删除前失败；不采用为事务型 DDL 实现或证据。 |

PG 自身限制与产品门禁分开：[CREATE DATABASE](https://www.postgresql.org/docs/18/sql-createdatabase.html) 和 [DROP DATABASE](https://www.postgresql.org/docs/18/sql-dropdatabase.html) 的 Notes 明确禁止事务块内执行，DROP 还禁止删除当前连接库；这些不是表级事务型 DDL 待开放能力。[CREATE INDEX](https://www.postgresql.org/docs/18/sql-createindex.html#SQL-CREATEINDEX-CONCURRENTLY) 区分普通建索引可在事务块内与 CONCURRENTLY 禁止事务块，不能把所有 DDL 一概类推。普通 [CREATE TABLE](https://www.postgresql.org/docs/18/sql-createtable.html)、[DROP TABLE](https://www.postgresql.org/docs/18/sql-droptable.html) 结合 [§3.4 Transactions](https://www.postgresql.org/docs/18/tutorial-transactions.html) 的原子性/中间状态隔离作为表级目标依据；DROP/ALTER/TRUNCATE 各自仍需专项证据，不由建表原型开放。

官方版本均为 **PostgreSQL 18**，访问日期 **2026-10-23**；这里规定目标，不声称已实测，也未证明 18.6 精确补丁行为/诊断。[§13.1 MVCC](https://www.postgresql.org/docs/18/mvcc-intro.html)、[§67.1 Transactions and Identifiers](https://www.postgresql.org/docs/18/transaction-id.html)、[§28.3 WAL](https://www.postgresql.org/docs/18/wal-intro.html) 分别定义快照、事务标识与日志恢复概念。不得用 TSO、连接计数或响应标签伪造 PG XID/xmin、快照可见性、WAL/LSN 或崩溃恢复；真实来源缺失须按第 1/3/6 章的字段能力明确拒绝或允许时返回有类型 SQL NULL，不能用固定值伪装。通过可观察原子性也不等于复制 PG 内部存储格式。

准入要求：固定两端版本/共享存储模式、真实 PG 工厂链、双连接目录观察及超时采证方法、事务前缀哨兵和独立测试入口，先取得当前拒绝基线；缺任何一项记录缺证。退出分为：全部最小探针与失败清理/共存取证通过，只允许提议该普通建表子集的后续实施；出现提前提交、泄露或残留则原型不通过、保留拒绝并记录底层限制；因目录/环境/观察无法取证则结论未知、保留拒绝并交具体缺证项。原型通过亦须后续真实实现及 Ready 证据后才能调整门禁，不能顺带解禁六类 DDL。

#### 章节验收与后续交接

下表将来源需求落到本章同名契约块及“验收”的具体场景；各行继承原块的完整夹具、两入口、元数据、副作用查询、恢复与 MySQL 用例，不缩减为标题检查。所有能力的运行 verdict 仍为**未验证**；有产品限制的分支另记拒绝基线，不能以该基线替代未来成功目标。

| 来源 → 能力/契约块 | 正例 / 反例的验收入口 | 副作用 → 恢复 → MySQL → 证据 |
| --- | --- | --- |
| 步骤 1/4 → SESSION-RETURNING/pg；返回子句执行契约 | session.returning.pg 的 insert-default/update-new/delete-old/zero-match/multi-row；gate 与 expression-error | returning_doc 完整行镜像和 S，独立提交/回滚；错误后 I 或 E→ROLLBACK；session.returning.mysql；pg_result::command 与 pg_dml_test 的拒绝意图，成功返回未证。 |
| 步骤 1/4 → SESSION-CONFLICT-NOTHING/pg；冲突忽略执行契约 | existing/fresh/all-keys/named/mixed；other-key/not-null/check/atomic | conflict_doc 的 S 证明旧行未覆盖、非仲裁错误未吞掉及批次撤销；显式失败恢复；本块 MySQL 自身 Query/prepared；buildInsert/InsertRuntime 与 insert_test 静态入口，PG仲裁未证。 |
| 步骤 1/4 → SESSION-CONFLICT-UPDATE/pg；冲突更新执行契约 | 本块普通更新、条件跳过与新插入；二次唯一错误、重复命中 | 旧行/候选行结果、实际修改行及完整 S；语句/事务回滚与新写继续；本块 MySQL 原生冲突行为；batchUpdateDupRows 为复用候选，不能当指定仲裁支持。 |
| 步骤 1/4 → SESSION-CTE-READ/pg；只读公共表表达式契约 | chain/repeat/shadow/nested/partial-alias；name-errors/recursive-gate | cte_read_doc 的 S 不变且作用域退出；失败前缀写入回滚；session.cte-read.mysql.coexist；目录 CTE 测试、buildWith/execute_with_clause 只提供局部证据。 |
| 步骤 1/4 → SESSION-CTE-WRITE/pg；写入公共表表达式契约 | move/empty/snapshot/repeat-consumer；destination-fail/source-fail/reject | 源/目的两份 S 同时检查，失败及 ROLLBACK 均不残留搬移；session.cte-write.mysql.coexist；现有 DELETE/CTE 接口未证明同语句快照和原子性。 |
| 步骤 2/4 → SESSION-SEARCH-PATH/pg；模式与搜索路径契约 | isolation/order/empty/commit/rollback；missing/gate/failed/local | path_doc 的 S、双方 SHOW、目录身份分别观测；恢复事务前路径并检查写入撤销；session.search-path.mysql；PgSession/Resolver 和 pg_session_test/pg_query_test 的有界意图。 |
| 步骤 2/4 → SESSION-NAME-CASE/pg；引号与大小写执行契约 | relation/column/alias/dot/write；缺对象、空引号、failed | t、"T"、"a.b" 不同标记值及写入 S；失败/回滚后各对象保持；session.name-case.mysql 及其 sql_mode/lower_case_table_names；名称改写测试不能替代后端精确对象证据。 |
| 步骤 2/4 → SESSION-PREPARED-STATE/pg；预处理执行时会话契约 | stable/path-switch/drop-recreate/bound-path/catalog-live；path-missing/ddl-shape/跨连接对象拒绝 | prep_doc 的 S、来源身份与固定描述，区分新 Bind 与旧 portal；Sync/ROLLBACK/重准备恢复；session.prepared-state.mysql；Statement/Portal 静态接线未证普通关系重绑定。 |
| 步骤 2/4 → SESSION-SEQUENCE/pg；序列执行契约 | first/cross/rollback/set-true/set-false；bounds/missing/type/permission | seq_rows 行回滚与序列消耗分开，currval 连接隔离；错误后继续及显式失败恢复；本块 MySQL 原生序列/auto-ID 基线；SequenceService/allocator 与 seq_autoid_test 不证明 PG 调用全链。 |
| 步骤 2/4 → SESSION-IDENTITY/pg；标识列生成契约 | ALWAYS/BY DEFAULT、DEFAULT及覆盖规则；显式值/NULL拒绝 | 实际生成行、RETURNING、列到真实序列的 i 依赖与消耗分离；普通行回滚不回收分配；本块 MySQL AUTO_INCREMENT 独立基线；ColumnInfo/目录测试未证 identity。 |
| 步骤 3/4 → SESSION-TXN/pg；事务提交与回滚契约 | commit/rollback/disconnect；repeated-begin/outside-end/error-recover | writer/observer 完整值及新快照，提交可见/整段撤销/关闭屏障；本块 MySQL 自身事务；prepared_dml_commit_and_rollback 仅有部分行数意图。 |
| 步骤 3/4 → SESSION-TXN-FAILED/pg；失败事务恢复契约 | 首错→E→拒绝读写→ROLLBACK→新事务；失败态COMMIT、预绑定portal、除零 | 首错前行写入撤销，原始错误与后续25P02分存；本块 MySQL 自身错误恢复；failed_transaction_requires_rollback 未覆盖全部阶段，COMMIT产品限制与PG目标分列。 |
| 步骤 3/4 → session.txn.sync.*；协议同步与事务恢复边界 | 显式/隐式成功段；Parse/Bind/Execute/活动取消错误段 | 成功前缀整段提交或撤销，丢弃段无副作用；Sync后显式E、隐式I分别检查；本块 MySQL 共存；Extended::handle 与既有协议测试不能证明隐式段原子性。 |
| 步骤 3/4 → SESSION-TX-DDL/pg；事务型建表能力门槛 | rollback/commit探针；reject/write-failure/database-restrict | 哨兵S、目录D、名称P联合证明对象/行及前缀原子性；故障/关闭清理与新写继续；session.txddl.mysql.coexist；独立原型仍未运行，保留门禁及底层不满足项。 |

公共执行链模板的 SESSION-DML-WRITE、SESSION-TX-DDL-GATE、SESSION-COEXIST 提供最小贯通用例，不取代上表十四族专项。每一族只有在自身 PG 参考、AsterSQL PG 与 MySQL 自身基线的原始记录齐全，且完整值、类型、错误阶段、真实副作用、恢复和共存均满足时才可讨论开放；语法接受、目录行、标签和现有测试名称均不计通过记录。

本次官方复核均访问于 **2026-10-23**，适用 **PostgreSQL 18 大版本**：DML 返回与冲突分别参见 [§6.4](https://www.postgresql.org/docs/18/dml-returning.html)、[INSERT](https://www.postgresql.org/docs/18/sql-insert.html)；CTE 行集与同快照参见 [§7.8](https://www.postgresql.org/docs/18/queries-with.html)；名称、路径及准备状态参见 [§4.1](https://www.postgresql.org/docs/18/sql-syntax-lexical.html)、[§5.10](https://www.postgresql.org/docs/18/ddl-schemas.html)、[PREPARE](https://www.postgresql.org/docs/18/sql-prepare.html)；序列/identity 参见 [§9.17](https://www.postgresql.org/docs/18/functions-sequence.html)、[§5.3](https://www.postgresql.org/docs/18/ddl-identity-columns.html)；普通行事务与同步参见 [§3.4](https://www.postgresql.org/docs/18/tutorial-transactions.html)、[§54.2](https://www.postgresql.org/docs/18/protocol-flow.html)。数据库DDL自身禁用项另核 [CREATE DATABASE](https://www.postgresql.org/docs/18/sql-createdatabase.html)、[DROP DATABASE](https://www.postgresql.org/docs/18/sql-dropdatabase.html)。这些是累计目标的有限规范依据，精细规则仍绑定各块官方引用；来源指定 **18.6** 的精确构建、补丁行为和诊断尚无运行证据，不从页面版本号推导差分通过。

### 范围外

#### 会话执行链与契约模板

本块只建立五节框架、执行地图、模板及其代表验收入口，不实现或修复生产/测试代码，不运行 Cargo/Go、数据库、差分平台或 DDL 原型，不改第 1–4、6–8 章及来源计划。RETURNING、ON CONFLICT、CTE、schema/search_path、prepared 状态时点、sequence/identity、正常/失败事务与 Sync 的完整规则由后块逐项补充。事务型 DDL 必须先证明共享引擎能满足原子性与回滚；当前拒绝保留，不能用响应标签模拟支持。此处不批准多语句、完整 savepoint、隔离级别组合或无 BEGIN 的扩展流水线事务等价性，也不借未测边界扩大本轮范围。

#### 返回子句执行契约

本块不实现 parser、planner、执行器、codec 或测试，不解除现有 RETURNING 门禁，不建设第二套 PG 引擎。序列/identity 的分配、非事务计数器回退由后续专项负责；本块生成值示例使用确定的列 DEFAULT 表达式，不承诺序列值在 ROLLBACK 后复用。ON CONFLICT、写入 CTE、MERGE、触发器/规则、UPDATE FROM/DELETE USING、继承表及并发写冲突不由普通表夹具覆盖。PG 18 的 OLD/NEW 与 `RETURNING WITH (OLD AS o, NEW AS n)` 是合法规范形式，后续单独取证和开放；未支持时在写入前拒绝，不能假装是 PG 非法语法。权限检查与更广的表达式集合沿相关工作流，不能因本块列出简单算术就承诺任意 SELECT 投影已可执行。

#### 冲突忽略执行契约

不实现冲突 AST、解析改写、目录补建、执行器或存储修改；DO UPDATE、excluded、更新条件交下一块。RETURNING 的投影/元数据沿前块；sequence/identity、事务恢复与 wire 精确字段分别交对应任务。表达式索引、collation/opclass、延迟/排斥约束、分区、触发器/规则及并发索引维护不在首批基本唯一键用例的开放证明内；合法但未支持形式须保留写前拒绝边界，不能悄悄降级。本文不运行原型、数据库实验、Cargo/Go 构建或性能测试，不用解析成功、标签或 MySQL IGNORE 证明 PG 副作用正确。

#### 冲突更新执行契约

不实现 parser、planner、执行器、存储或测试，不移除任何门禁，不补建 PG 引擎。复用前块目标推断，表达式/部分索引、触发器/规则、分区键迁移、排斥/延迟约束、完整权限、sequence/identity、OLD/NEW、写入 CTE 均不由基本表正例开放。并发计划只规定证据，不运行平台、数据库、Cargo/Go 或性能实验；合法而未支持与 PG 本身非法形式分别记录，不从 MySQL 近似语法得出 PG 支持结论。

#### 只读公共表表达式契约

不实现或修改 parser、名称适配、planner、执行器、目录、协议及测试，不把目录 CTE 限额移植为应用 SQL 的兼容限制，不补建另一套 PG 引擎。写入 CTE、主 DML、volatile/序列副作用、行锁、递归与 SEARCH/CYCLE、显式物化的完整执行及优化/溢写成本均不由本块正例开放。共享路径的别名数、求值策略与 PG 可能存在差异，只登记后续取证边界，不在文档任务修复；不修改第 2 章名称规则或其他章节，不运行数据库、Cargo/Go、差分平台或性能实验。

#### 写入公共表表达式契约

不实现 parser、planner、执行器、事务、协议或测试，不解除现有 RETURNING/递归门禁，不另建 PG 执行引擎。仅为普通表 DELETE→INSERT 搬移及必要拒绝/快照证据建立契约；MERGE、递归键集驱动写入、触发器/规则、分区/继承、ON CONFLICT 组合、sequence/identity 与权限完整矩阵另行取证。无 RETURNING、未消费及兄弟分支案例是语义门槛，不由主搬移正例自动开放；对同一行重复写入不规定确定性结果。共享内核未满足的快照/原子性列为开放限制，不在本任务递归补建子系统，不运行数据库、Cargo/Go、差分平台或性能实验。

#### 模式与搜索路径契约

本块仅补连接路径与对象可见性文档；不实现多 schema、角色/权限、临时对象、用户函数/重载、ALTER ROLE/DATABASE 配置、set_config 或目录系统。真实 schema/OID 与权限由工作流 6 提供；普通 prepared 名称绑定、重分析与缓存时点交本章预处理专项，不能把 current_schema 标量的 Execute 测试外推为所有关系重绑定。savepoint、函数局部设置、事务型建表及完整事务状态机交各专项；不以路径生命周期要求扩写共享引擎。本轮不运行数据库、差分平台、Cargo/Go 或 DDL 原型，不改其他章节与来源任务。

#### 引号与大小写执行契约

只补实际对象选择契约；不实现 parser、名称映射、大小写敏感目录/存储改造、多 schema、权限、用户函数或完整 Unicode/名称截断规则，不改全局配置。路径生命周期复用前块；prepared 重分析/缓存时点归下一专项，本块扩展查询均新建 statement/portal。事务型 DDL、sequence/identity、跨库引擎与 MySQL 模式实现不在范围；不运行数据库、差分平台、Cargo/Go 或原型，不改第 1–4、6–8 章。

#### 预处理执行时会话契约

本块不实现 prepared 缓存、失效广播、PG resolver、类型系统或 schema 子系统；不补 generic/custom plan 策略或性能基准，不开放事务型 DDL、任意多 schema、二进制格式及通用参数推断。SQL PREPARE/EXECUTE 仅作官方语义来源，本轮用例走扩展协议，不据此宣称产品支持 SQL PREPARE。事务恢复、Close/Sync/portal 精确生命周期沿专项和工作流 7；目录真实对象、权限与可见性归工作流 6。目录 Execute 重新取快照不承担原生 prepared handle 的失效证明。

#### 序列执行契约

不实现序列函数、权限、regclass、分配器、存储或测试，不改原生 MySQL 行为，不补建另一套 PG 引擎。序列 DDL 只作为未来验收的事务外夹具前提，不借本块批准事务型 CREATE/ALTER/DROP SEQUENCE、RESTART 或解除现有门禁。默认值稳定引用、OWNED BY、serial/identity 及删除级联交目录工作流与 identity 后块；缓存失效、多节点/崩溃恢复和全类型/NULL/极值组合须另取证。本轮不运行数据库、差分平台、原型、Cargo/Go 或性能实验。

#### 标识列生成契约

只补 identity 的插入、目录依赖与验收文档，不实现 parser、planner、执行器、元数据、测试或另一套 PG 引擎。CREATE TABLE 仅作为未来事务外夹具，不解除事务内 DDL 拒绝；ALTER identity、RESTART、DROP/级联、serial 完整语法、UPDATE identity、COPY、分区/继承、触发器、权限矩阵与 ON CONFLICT/写入 CTE 组合分别后续取证。序列并发、缓存/崩溃恢复沿前块，不因单连接 CACHE 1 数值例开放；本轮不运行数据库、差分平台、Cargo/Go、原型或性能实验，不改其他章节及来源计划。

#### 事务提交与回滚契约

不实现状态机、事务管理器或数据库实验；不承诺 SAVEPOINT/子事务、AND CHAIN、两阶段提交、全部隔离模式、锁/死锁/重试、取消竞态、提交结果不确定时的幂等策略、崩溃恢复或 WAL/XID 内部等价。序列取值和 identity 分配的非回滚语义沿前块，不将“全部撤销”扩大到序列；事务型 DDL 沿能力门槛保留产品拒绝。失败事务与 Sync 的完整状态转换分别交批次 13、14，DDL 能力门槛交批次 15；不修改 MySQL 既有事务规则。

#### 失败事务恢复契约

不实现失败状态机、补 Execute 门禁、改变现有 COMMIT 拒绝策略或运行数据库实验。SAVEPOINT、ROLLBACK TO、RELEASE、ABORT/WORK/TRANSACTION 等别名与变体、AND CHAIN、嵌套子事务及错误回滚的并发/取消竞态均是独立后续边界；不以放宽字符串门禁代替子事务能力。隐式事务批处理及完整 Sync 消息矩阵交批次14，事务型 DDL 交批次15；不建设第二套引擎、不伪造 PG XID/WAL、不改变 MySQL 原有语句错误和事务继续规则。

#### 协议同步与事务恢复边界

不实现Sync提交/回滚接线、事务门禁或取消，不搭建协议状态机、驱动流水线、数据库/差分平台；不将当前Sync无事务调用直接定为运行缺陷。COPY、特殊DDL的提前提交、多个同步段/COMMIT交错、保存点、输出阶段或提交时失败、网络中断与取消提交竞态另取窄证据，不能套用本块普通DML断言。完整wire消息、错误优先级、Close/portal/statement生命周期及取消密钥归工作流7，真实目录归6，双方言客户端归8；本轮不改第1–4及6–8章。

#### 事务型建表能力门槛

本轮不运行或实现原型，不改 Rust/Go、DDL job/owner、parser、存储、目录或协议，不调用 Cargo/Go/数据库实验；只交付文档。普通建表探针不覆盖完整事务 DDL、临时/UNLOGGED 表、CREATE TABLE AS、分区、外键/触发器、序列/identity、savepoint、并发同名 DDL、跨库、集群故障或崩溃持久性。CREATE/DROP DATABASE 等 PG 本身禁用项另保留限制；DROP VIEW 等未列门禁路径仅调查，不能凭空判为支持或确定缺陷。缺共享元数据原子性时列为独立可行性限制，不递归补建完整 DDL 或第二引擎，不用 MySQL DDL 近似语法证明 PG 等价；不修改前序块、第 1–4/6–8 章及来源计划。

#### 章节验收与后续交接

本块只完成覆盖审阅与交接，不执行代码移植、Cargo/Go构建、数据库实验、差分平台、DDL原型或性能测试；不生成实现任务、不开放能力、不替上层任务标完成，不改第1–4、6–8章及来源状态。扩展加载、第二套引擎、完整多schema/权限/缓存/事务/DDL系统仍在范围外。保存点、递归/物化、并发仲裁、CACHE>1、多节点、真实存储故障与崩溃持久性各保留独立入口，不能在章节收口时递归补全。

底层不能证明的元数据/行原子性、真实对象引用、固定类型或事务恢复继续列为能力门槛；TSO不替代PG XID/xmin，缺少真实来源不得伪造WAL/LSN/MVCC值。DDL原型通过也仅能提议普通建表子集实施，不自动解除其他DDL拒绝。当前源码风险不是已经复现的运行缺陷；未采的PG/MySQL结果、精确SQLSTATE/位置和18.6参考行为保持未验证。

### 验收

#### 会话执行链与契约模板

以下是**待新增用例定义，全部未运行**，用于检验模板能表达 DML 写入和事务拒绝，不是实现支持清单。每个变体/消息模式分别分配 case_id；全部使用 UTF-8、UTC、独立无业务数据的测试数据库和空闲连接，无额外客户端隐式 BEGIN。观察连接每次 SELECT 使用新的自动提交语句，记录实际隔离级别，不能用旧快照制造可见性结论。PG 使用 public，MySQL 使用自身当前数据库；准备/清理均在事务外逐条发送 SQL，不组合成一个 Q。

| 能力与用例 | 完整输入、初始状态与目标 | 副作用、恢复和取证缺口 |
| --- | --- | --- |
| SESSION-DML-WRITE/pg；session.dml-write.pg.simple-commit、session.dml-write.pg.extended-commit、session.dml-write.pg.extended-rollback；分类为有界共享语义候选 | PG 写连接 W 与观察连接 O 同库、I；准备 `CREATE TABLE public.session_chain_doc (id integer PRIMARY KEY)`，初始 `SELECT id FROM public.session_chain_doc ORDER BY id` 为零行。提交变体 W 逐条 `BEGIN`、`INSERT INTO public.session_chain_doc VALUES (1)`、`COMMIT`；扩展变体在 BEGIN 前 Parse 命名 s：`INSERT INTO public.session_chain_doc VALUES ($1)`、OID [23]，再 Q BEGIN、Bind 新 portal p、text 参数 1、无结果格式要求，Execute p(max_rows=0)，Sync，最后 Q COMMIT；回滚变体独立重建空表，值换 2 并以 Q ROLLBACK 结束。 | INSERT 无结果列/返回行（未指定 RETURNING），标签与影响行数单列采集；W 提交前 SELECT 是 [1] 或 [2]，O 是零行；提交后 W/O 均 [1]，回滚后 W/O 均零行。SELECT 列 id 为非 NULL int4、OID 23/typlen 4/typmod -1、text；BEGIN 后 T，显式事务内 Sync 后 T，COMMIT/ROLLBACK 后 I。成功 SQLSTATE 不适用。清理先确保 W ROLLBACK，再关闭 s/p（若仍存在）、事务外 `DROP TABLE public.session_chain_doc`、关 W/O；精确标签/元数据实测待采，现有 prepared_dml_commit_and_rollback 仅提供部分行数断言意图。 |
| SESSION-TX-DDL-GATE/pg；session.tx-ddl-gate.pg.simple、session.tx-ddl-gate.pg.extended；分类为明确不支持（AsterSQL 当前限制） | 独立空库 W/O 为 I，事务外准备 `CREATE TABLE public.session_gate_doc (id integer PRIMARY KEY)`，确认 `SELECT id FROM public.session_gate_doc ORDER BY id` 零行；W Q BEGIN、Q `INSERT INTO public.session_gate_doc VALUES (7)`，再发 `CREATE TABLE public.session_gate_new (id integer)`；简单变体用 Q，扩展变体在 T 时发 Parse 命名 blocked，参数 OID 数为 0，再发 Sync。 | AsterSQL 限制目标 0A000、无成功行/对象，状态 E；随后 Q `SELECT 1 AS v` 目标 25P02/E；扩展错误后先 Sync，仍 E，再发该 Q，不能将被丢弃的 Q 算门禁证据。W ROLLBACK 后 I、Q `SELECT 1 AS v` 一行非 NULL 1/int4（23/4/-1）；W/O 对原表 SELECT 均零行，`SELECT id FROM public.session_gate_new` 应对象不存在且无成功数据，精确诊断/字段待参考及产品采集。清理确保 ROLLBACK、事务外 `DROP TABLE IF EXISTS public.session_gate_new` 与 `DROP TABLE public.session_gate_doc`，关连接。参考 PG 普通 CREATE TABLE 事务行为另采，不能要求它报此产品限制；DDL 能力原型未过不解除门禁。 |
| SESSION-COEXIST/mysql；session.coexist.mysql.query、session.coexist.mysql.prepared，关联上述 PG case_id；分类为 MySQL 专属入口保留契约 | 同一 AsterSQL Server 上另开 MySQL M/N，使用独立测试数据库，先在 M/N 各执行 `SET autocommit=1`，记录 sql_mode、autocommit 及实际隔离级别，先采 Query `SELECT 1 AS v` 与 M 的 Prepare `SELECT ? AS v`、Execute 参数整数 1（驱动类型及 binary 格式记录原值）。准备 `CREATE TABLE session_mysql_doc (id INT PRIMARY KEY)`；M 逐条 `BEGIN`、`INSERT INTO session_mysql_doc VALUES (9)`，N `SELECT id FROM session_mysql_doc ORDER BY id`，M `COMMIT`，N 重查。PG W 在 M 事务期间交错执行上方拒绝与恢复。 | N 提交前零行、提交后一行 [9]；PG 的失败/ROLLBACK 不结束 M 事务、不改 M 的模式/当前库、参数或数据。PG 前后重跑 M 的 Query 与原 prepared Execute（1，再换 SQL NULL），逐项对照自身 Go/TiDB/MySQL 参考及此前基线，保留值/NULL、类型标志/charset/length/decimals、code/SQLSTATE/警告；精确元数据未采不臆造为 PG int4。清理 M `ROLLBACK`、关闭 prepared、事务外 `DROP TABLE session_mysql_doc`、关 M/N。此用例不在 MySQL 发送 PG DDL/RETURNING 近似语句，也不推断 MySQL 失败事务与 PG 等价。 |

拒绝用例还须记录写入前后及恢复后的实际数据，若对象不存在查询自身触发错误，应在事务外独立执行并记录其可继续性，不能污染主用例。PG 两种模式分别采消息、错误字段与状态；没有结果集的命令其列类型不适用，失败也不生成伪造 RowDescription。需要 RETURNING 的 DML 返回类型由后续专项补证，不能用上表无返回行的 INSERT 代表已覆盖。原始运行 observations 四端均为“未采集”，verdict 为“未验证”；参考版本、命令/退出码及有效断言数在真实运行时补齐，当前有效运行数 0。

人工文档验收：五节各有本块同名内容；来源步骤 1 的 DML 公共接入对应 SESSION-DML-WRITE，步骤 2 的名称/会话边界对应模块地图与跨工作流责任，步骤 3 的状态/Sync/DDL 门槛对应 SESSION-TX-DDL-GATE，步骤 4 的副作用、恢复与 MySQL 共存对应三组用例和模板。专项 SQL 细节继续分块，未声称完成来源整个工作流。正确性风险是专用分支或已准备 portal 可能未满足完整状态门禁；兼容性风险是产品限制/类型/错误与参考端不一致；均待实施取证。性能未测试，无吞吐、延迟或流式执行结论。

#### 返回子句执行契约

以下均为**待新增用例定义，未运行**，沿本章模板保存 observations/evidence_refs/verdict。用例前缀 `session.returning.pg.`；表中短名分别扩为 `.simple-rollback`、`.extended-rollback`、`.simple-commit`、`.extended-commit`，各为独立 case_id，能力均为 SESSION-RETURNING/pg。全部使用同库独立 W/O 两连接、UTF-8、UTC、初始 I、无触发器/规则及并发写入；记录实际隔离级别，O 每次 SELECT 为新自动提交语句。

每例事务外逐条准备 `CREATE TABLE public.returning_doc (id integer PRIMARY KEY DEFAULT (40 + 2), v integer, note text)`，再执行各行初始 INSERT（无则不插入）。统一检查 SQL 为 `SELECT id, v, note FROM public.returning_doc ORDER BY id`，以下简称 S；在 W/O 上均先执行 S 核对初始状态。W 逐条 BEGIN、目标 DML、S；O 执行 S 仍为初始值。回滚变体 W ROLLBACK 后 W/O 各执行 S 均恢复初始值；提交变体独立重建同一初始状态并重跑，COMMIT 后 W/O 的 S 均为写后值。BEGIN 后 T，成功 Execute 后 Sync 仍 T，事务结束 I。清理每例先 W ROLLBACK、关闭仍存在的 portal/statement，再事务外 DROP TABLE public.returning_doc，关闭 W/O；每条 Q 只含一条语句。

| 用例短名、完整目标 SQL 与初始输入 | 返回行/列及独立标签目标 | 写后 S；ROLLBACK 后 S（W/O 均检查） |
| --- | --- | --- |
| insert-default；初始空表；`INSERT INTO public.returning_doc (v, note) VALUES (7, NULL) RETURNING id AS generated_id, v, note` | 列 generated_id/v/note，一行 (42,7,SQL NULL)；INSERT 0 1。42 由 DEFAULT 求值得到，不是客户端回显。 | 写后 [(42,7,SQL NULL)]；回滚后空表。 |
| update-new；初始 `INSERT INTO public.returning_doc VALUES (1,10,'before')`；`UPDATE public.returning_doc AS r SET v = v + 1, note = NULL WHERE id = 1 RETURNING r.id, r.v AS "After", r.note, r.v + 100 AS projected` | 列 id/After/note/projected，一行 (1,11,SQL NULL,111)；UPDATE 1。111 仅是返回表达式值，存储 v 为 11。 | 写后 [(1,11,SQL NULL)]；回滚后 [(1,10,'before')]。 |
| delete-old；初始 `INSERT INTO public.returning_doc VALUES (1,10,'before')`；`DELETE FROM public.returning_doc AS r WHERE id = 1 RETURNING r.*` | 按表列序 id/v/note，一行 (1,10,'before')；DELETE 1。不能从删除后的空表补造旧行。 | 写后空表；回滚后 [(1,10,'before')]。 |
| zero-match；初始 `INSERT INTO public.returning_doc VALUES (1,10,'before')`；`UPDATE public.returning_doc SET v = 99 WHERE id = 9 RETURNING id, v, note` | id/v/note 描述固定，零 DataRow；UPDATE 0。扩展 Describe 仍有列，不能给 NoData。 | 写后和回滚后均 [(1,10,'before')]。 |
| multi-row；初始 `INSERT INTO public.returning_doc VALUES (1,10,NULL),(2,20,'b')`；`UPDATE public.returning_doc SET v = v + 1 RETURNING id, v, note` | 列 id/v/note，多重集 {(1,11,SQL NULL),(2,21,'b')}；UPDATE 2，无默认返回顺序。 | 写后 [(1,11,SQL NULL),(2,21,'b')]；回滚后 [(1,10,SQL NULL),(2,20,'b')]。 |

简单变体使用表中 SQL。扩展变体在 BEGIN 前 Parse 命名 s、Describe statement；BEGIN 后 Bind 新 portal p（text 输入与结果格式）、Describe portal、Execute p(max_rows=0)、Sync，再做 S 与事务结束；Parse/两种 Describe 前后 W/O 的 S 均不得变化，输出类型按目标行为段逐列断言，成功 SQLSTATE 不适用。update-new 扩展变体改用完整 SQL `UPDATE public.returning_doc AS r SET v = v + $1, note = $2 WHERE id = 1 RETURNING r.id, r.v AS "After", r.note, r.v + $3 AS projected`，参数 OID [23,25,23]、值 [1,SQL NULL,100]、格式 [0,0,0]；其余例 OID/参数为空。另建 `session.returning.pg.update-same.extended-rollback`，同 update-new 初始值，执行 `UPDATE public.returning_doc SET v = v WHERE id = 1 RETURNING id, v`，目标行 (1,10)、UPDATE 1；写后及回滚后 S 均保持 (1,10,'before')，专门区分匹配计数和改值计数。

拒绝与失败用例亦为待新增，分别设 `.simple`、`.extended` case_id；沿用表结构、S、连接与清理规则，初始 `INSERT INTO public.returning_doc VALUES (1,10,'before')`。简单变体在所述时点发送 Q；扩展变体在同一时点 Parse 命名 s（OID/参数为空），成功才继续 Bind p（空参数、text 结果）、Describe portal、Execute p(max_rows=0)，遇错即 Sync；运行时错误例须 Parse/Bind 成功后到 Execute，不能把 Parse 拒绝当成求值失败：

- `session.returning.pg.gate-{insert,update,delete}`：分别发送 `INSERT INTO public.returning_doc VALUES (2,20,'blocked') RETURNING id`、`UPDATE public.returning_doc SET v = 99 WHERE id = 1 RETURNING v`、`DELETE FROM public.returning_doc WHERE id = 1 RETURNING id`。作为**现有产品限制**验收时目标 0A000、无成功标签/返回行；PG 参考端应成功，不能以参考端拒绝作为通过条件。I 中错误后（扩展先 Sync）仍 I，S 必须为 [(1,10,'before')]，再 `SELECT 1 AS ok` 成功。各自独立重建初始值，不能在空表上验证更新/删除保护。支持开放后这些输入转为成功目标，门禁用例迁移到仍未支持的合法形态。
- `session.returning.pg.old-new-gate`：W BEGIN 后先 `INSERT INTO public.returning_doc VALUES (3,30,'pending')`，再发送合法 PG 输入 `UPDATE public.returning_doc SET v = 99 WHERE id = 1 RETURNING WITH (OLD AS o, NEW AS n) o.v, n.v`。尚未开放此形式时 AsterSQL 目标在写入前 0A000，无行/成功标签；当前更早解析是否满足此诊断仍未验证。O 的 S 仅 [(1,10,'before')]；错误后扩展先 Sync 仍 E，Q `SELECT 1 AS ok` 目标 25P02/E，W ROLLBACK 后 I，W/O 的 S 均仅初始行，确认 pending 也被撤销。PG 参考端目标返回 (10,99)，W 写后 S 为 [(1,99,'before'),(3,30,'pending')]，O 仍仅初始行；ROLLBACK 后 W/O 的 S 均仅初始行，不与产品限制混为同一结论。
- `session.returning.pg.expression-error`：仅在基本 RETURNING 能力开放后运行，W BEGIN，执行 `UPDATE public.returning_doc SET v = 11 WHERE id = 1 RETURNING 10 / (v - 11) AS boom`；PG 目标运行时 22012、无成功 CommandComplete。O 的 S 保持初始值；扩展先 Sync 仍 E，下一 Q `SELECT 1 AS ok` 25P02，ROLLBACK 后 I，W/O 的 S 均 [(1,10,'before')]，再 SELECT 1 成功。另建独立无 BEGIN 变体，错误及 Sync 后 I，直接 S 仍初始值，证明语句原子性；实际错误发生阶段、消息 S/V/C/M/D/H/P 与是否已输出部分行均待参考/产品采集，尚处门禁阶段的 0A000 不能算此运行错误用例通过。

MySQL 共存能力 `SESSION-RETURNING/mysql` 分类为 MySQL 专属入口保留契约，用例 `session.returning.mysql.{query,prepared}` 与上述 PG case_id 通过 related_case_ids 关联。在同一 Server 开 M/N，独立当前库，SET autocommit=1，记录 sql_mode/隔离级别；事务外 `CREATE TABLE returning_mysql_doc (id INT PRIMARY KEY, v INT, note TEXT)` 并插入 (1,10,'before')，先保存 Query `SELECT id,v,note FROM returning_mysql_doc ORDER BY id` 及 Prepare `SELECT v,note FROM returning_mysql_doc WHERE id = ?`、Execute 整数 1 的完整自身元数据/值。M BEGIN，Query `UPDATE returning_mysql_doc SET v = v + 1 WHERE id = 1`，M 查询值 11、N 查询仍 10；期间交错 PG 成功目标或门禁/失败恢复，再执行原 MySQL prepared（仍得 11/'before'）并核对模式、库和事务未变，M ROLLBACK 后 M/N 查询均 (1,10,'before')。独立重建后另做 MySQL prepared UPDATE 同 SQL 的 `WHERE id = ?`（参数 1），重复写后/回滚检查。PG 前后 MySQL 查询、类型标志/charset/length/decimals、错误/警告各对自身基线和 Go/TiDB/MySQL 参考，精确字段待采；不以 MySQL 近似 RETURNING 或 changed-rows 数字证明 PG 等价。清理 M ROLLBACK、关闭 prepared、事务外 DROP TABLE returning_mysql_doc、关闭 M/N。

人工验收映射：来源步骤 1 RETURNING → 五类正例的返回镜像/生成值/零行/多行；步骤 4 副作用与恢复 → 每例 S、独立提交变体、拒绝与表达式失败；双方言隔离 → MySQL Query/Prepare/Execute。当前所有 observations 为未采集、verdict 为未验证，wire 精确来源字段/错误位置、真实持久可见性及 18.6 差分均未取证。正确性风险在返回镜像、表达式失败后的原子性与重复执行；兼容性风险在类型/别名、计数及合法形式的拒绝阶段；性能未测，不作缓存、流式输出或吞吐结论。

#### 冲突忽略执行契约

以下均为**待新增**用例定义，非已执行测试。case_id 使用 `session.conflict-nothing.pg.<短名>.<simple|extended>.<commit|rollback>`；错误/门禁例用 `<simple|extended>.<idle|transaction>`，能力为 SESSION-CONFLICT-NOTHING/pg，related_case_ids 关联本块 MySQL 用例。沿公共模板记录 UTF-8、UTC、实际隔离级别、独立同库 W/O、初始 I、客户端自动提交与无并发写入/触发器/规则。每个用例独立事务外准备，Q 每次单语句：

```sql
CREATE TABLE public.conflict_doc (
  id integer CONSTRAINT conflict_doc_pkey PRIMARY KEY,
  email text CONSTRAINT conflict_doc_email_key UNIQUE,
  v integer NOT NULL CHECK (v > 0)
);
INSERT INTO public.conflict_doc VALUES (1,'a',10),(2,'b',20);
SELECT id,email,v FROM public.conflict_doc ORDER BY id;
```

最后一条简称 S，初始完整结果 B=[(1,'a',10),(2,'b',20)]。各成功例 W BEGIN 后执行表中完整 SQL（返回列均 id/email/v，int4/text/int4，text 格式；精确来源/typmod 等沿 RETURNING 契约采集），再 W/O 各 S。表中“写后”指 W；O 提交前始终 B。W ROLLBACK 后双方 S 均 B；独立重建并重跑 COMMIT 变体后双方 S 均为表中写后值。成功 SQLSTATE 不适用，BEGIN 后 T、Execute/Sync 后 T、事务结束 I。

| 短名与完整 SQL（均以上述 B 起始） | 返回行；修改数/标签 | 写后 S |
| --- | --- | --- |
| existing：`INSERT INTO public.conflict_doc VALUES (1,'changed',99) ON CONFLICT (id) DO NOTHING RETURNING id,email,v` | 零行；0 / INSERT 0 0 | B，旧 email/v 不覆盖 |
| fresh：`INSERT INTO public.conflict_doc VALUES (3,'c',30) ON CONFLICT (id) DO NOTHING RETURNING id,email,v` | (3,'c',30)；1 / INSERT 0 1 | B 加 (3,'c',30) |
| all-keys：`INSERT INTO public.conflict_doc VALUES (3,'b',30) ON CONFLICT DO NOTHING RETURNING id,email,v` | 零行；0 / INSERT 0 0 | B，确证非主键也被选择 |
| named：`INSERT INTO public.conflict_doc VALUES (3,'b',30) ON CONFLICT ON CONSTRAINT conflict_doc_email_key DO NOTHING RETURNING id,email,v` | 零行；0 / INSERT 0 0 | B |
| mixed：`INSERT INTO public.conflict_doc VALUES (1,'changed',99),(3,'c',30) ON CONFLICT (id) DO NOTHING RETURNING id,email,v` | 仅 (3,'c',30)；1 / INSERT 0 1 | B 加 (3,'c',30) |

各例另设不带 RETURNING 的独立变体（短名加 `-no-returning`），去掉末尾 RETURNING id,email,v，标签/最终行相同但无结果集，避免当前 RETURNING 限制遮蔽冲突契约。simple 发送 Q；extended 在 BEGIN 前 Parse s、Describe statement（无参数/OID），BEGIN 后 Bind 新 portal p（空参数、text 结果）、Describe portal、Execute(max_rows=0)、Sync；Parse/Describe 前后 S 均 B。fresh 的参数化独立例使用 `INSERT INTO public.conflict_doc VALUES ($1,$2,$3) ON CONFLICT (id) DO NOTHING RETURNING id,email,v`，OID [23,25,23]、text 值 [3,'c',30]。任何早期能力拒绝只能记限制，不能计成功验收。

错误例每次重建 B，执行以下完整输入；默认无 RETURNING，错误无返回行/成功标签、修改数不记成功 0。SQLSTATE 的单例精确值、消息 S/V/C/M/D/H/P、发生阶段与位置均待参考端采集，不能从附录类别或文本推断已通过：

- other-key：`INSERT INTO public.conflict_doc VALUES (3,'b',30) ON CONFLICT (id) DO NOTHING`，仅 email 唯一冲突，须失败；不能忽略成成功。
- bad-target：`INSERT INTO public.conflict_doc VALUES (3,'c',30) ON CONFLICT (v) DO NOTHING`，v 无唯一索引，即使数据无冲突仍推断失败；bad-name：`INSERT INTO public.conflict_doc VALUES (3,'c',30) ON CONFLICT ON CONSTRAINT missing_constraint DO NOTHING`，名称失败。
- not-null：`INSERT INTO public.conflict_doc VALUES (3,'c',NULL) ON CONFLICT DO NOTHING`；check：`INSERT INTO public.conflict_doc VALUES (3,'c',0) ON CONFLICT DO NOTHING`；expression：`INSERT INTO public.conflict_doc VALUES (3,'c',1/0) ON CONFLICT DO NOTHING`。各自唯一键无冲突，必须保留独立错误；不得降为警告或写入替代值。
- atomic：`INSERT INTO public.conflict_doc VALUES (3,'c',30),(4,'b',40) ON CONFLICT (id) DO NOTHING`，第二行非目标唯一错误须撤销本语句第一行，最终 S 仍 B。

idle 变体 I 中发送 Q 或 Parse/Bind/Execute（零参数、text），错误后扩展先 Sync，Ready I，双方 S 仍 B，SELECT 1 成功。transaction 变体 W BEGIN 后先 `INSERT INTO public.conflict_doc VALUES (9,'pending',90)`，再发错误输入；运行错误须到 Execute，不能把 Parse 拒绝计为执行错误通过。O 的 S 始终 B；出错后扩展 Sync 仍 E，W Q `SELECT 1` 目标 25P02/E，W ROLLBACK 后 I，W/O 的 S 均 B，pending 和语句中间写入均未残留，再 SELECT 1 成功。产品对尚未开放的合法输入单设 gate case，目标写前 0A000、按同样恢复及 S 检查，参考端则按成功表取证；当前是否正确门禁未验证。

部分索引独立用例 `partial` 另在事务外建 `CREATE TABLE public.conflict_partial (id integer PRIMARY KEY,k integer,active boolean)`、`CREATE UNIQUE INDEX conflict_partial_active ON public.conflict_partial(k) WHERE active`，执行 `INSERT INTO public.conflict_partial VALUES (1,7,true)`。W BEGIN 后逐条执行 `INSERT INTO public.conflict_partial VALUES (2,7,true) ON CONFLICT (k) WHERE active DO NOTHING RETURNING id`（零行/INSERT 0 0）、`INSERT INTO public.conflict_partial VALUES (3,7,false) ON CONFLICT (k) WHERE active DO NOTHING RETURNING id`（返回 3/INSERT 0 1）；S 改 `SELECT id,k,active FROM public.conflict_partial ORDER BY id`，W 写后 [(1,7,true),(3,7,false)]、O 仍 [(1,7,true)]。分别提交后双方写后值、回滚后双方初始值。用无 RETURNING 变体隔离能力；独立重建后执行 `INSERT INTO public.conflict_partial VALUES (2,7,true) ON CONFLICT (k) DO NOTHING`，须推断失败、不写入，恢复同上。产品无部分索引能力时记录 setup/执行的明确限制，不以 setup 拒绝替代 PG 成功证明。

MySQL 共存用例 `session.conflict-nothing.mysql.<query|prepared>`，能力 SESSION-CONFLICT-NOTHING/mysql，按自身 Go/TiDB/MySQL 基线验收，不将 INSERT IGNORE 当等价实现。M/N 同 Server 独立当前库，事务外 `CREATE TABLE conflict_mysql_doc (id INT PRIMARY KEY,email VARCHAR(30) UNIQUE,v INT NOT NULL)`、`INSERT INTO conflict_mysql_doc VALUES (1,'a',10),(2,'b',20)`，SET autocommit=1 并记录 sql_mode、隔离级别。M BEGIN，Query `INSERT IGNORE INTO conflict_mysql_doc VALUES (1,'changed',99),(3,'c',30)`；prepared 变体独立重建，Prepare `INSERT IGNORE INTO conflict_mysql_doc VALUES (?,?,?),(?,?,?)`，Bind/Execute 同六值及自身整数/字符串参数类型。M `SELECT id,email,v FROM conflict_mysql_doc ORDER BY id` 目标 B 加 (3,'c',30)，N 同查询仍 B；交错 PG 成功/失败恢复后重复双方查询及原 prepared SELECT `SELECT v FROM conflict_mysql_doc WHERE id=?`（1→10），元数据、模式、警告、affected rows 对自身基线采集，不套 PG 数字。ROLLBACK 后 M/N 均 B；独立提交变体 COMMIT 后均写后值。警告精确条目/错误码尚未采集。

所有例清理先 ROLLBACK、Close 存在的 portal/statement/prepared，事务外 DROP 各自 conflict_doc/conflict_partial/conflict_mysql_doc，再关闭连接；避免在失败事务中 DROP。来源步骤 1→目标选择和成功表，步骤 4→S、提交/回滚、错误恢复和 MySQL 交错。observations 未采、verdict 未验证，运行用例 0；正确性风险为错误过滤、原子性、索引选择，兼容性风险为合法目标的拒绝阶段、诊断和零行返回元数据，性能未测。

#### 冲突更新执行契约

以下均为**待新增**用例定义，不是已运行测试。case_id 为 `session.conflict-update.pg.<短名>.<simple|extended>.<commit|rollback>`；错误例末段用 idle/transaction；能力 SESSION-CONFLICT-UPDATE/pg，related_case_ids 关联下述 MySQL 对照。公共模板的 observations/verdict 均为未验证。W/O 为同库独立连接，UTF-8、UTC、初始 I、客户端 autocommit、记录实际隔离级别；基本例无触发器/规则、并发写入。每例独立在事务外准备（每条 Q 仅一条 SQL）：

```sql
CREATE TABLE public.upsert_doc (
  id integer PRIMARY KEY,
  email text UNIQUE,
  v integer NOT NULL DEFAULT 7 CHECK (v > 0)
);
INSERT INTO public.upsert_doc VALUES (1,'a',10),(2,'b',20);
SELECT id,email,v FROM public.upsert_doc ORDER BY id;
```

最后查询简称 S，初始 B=[(1,'a',10),(2,'b',20)]。每个成功例 W BEGIN 后执行下表完整 SQL，W/O 各 S；表中写后值仅 W 可见，O 仍 B。ROLLBACK 后双方 B；独立重建的 COMMIT 变体双方为写后值。成功 SQLSTATE 不适用，BEGIN/Execute/Sync 后 T，结束 I。返回列顺序 id/email/v，int4/text/int4（OID 23/25/23），text 格式，其他描述字段沿 RETURNING 块；多行按带主键多重集比较。

| 短名与完整输入 | 返回；计数/标签；W 写后 S |
| --- | --- |
| update：`INSERT INTO public.upsert_doc AS dst VALUES (1,'candidate',3) ON CONFLICT (id) DO UPDATE SET v=dst.v+excluded.v RETURNING id,email,v` | (1,'a',13)；1/INSERT 0 1；[(1,'a',13),(2,'b',20)]，保留旧 email，排除普通插入或候选覆盖。 |
| fresh：`INSERT INTO public.upsert_doc AS dst VALUES (3,'c',3) ON CONFLICT (id) DO UPDATE SET v=dst.v+excluded.v RETURNING id,email,v` | (3,'c',3)；1/INSERT 0 1；B 加 (3,'c',3)，不执行更新表达式。 |
| default：`INSERT INTO public.upsert_doc AS dst (id,email) VALUES (1,'candidate') ON CONFLICT (id) DO UPDATE SET v=dst.v+excluded.v RETURNING id,email,v` | (1,'a',17)；1/INSERT 0 1；[(1,'a',17),(2,'b',20)]，证明 excluded 含 DEFAULT 7。 |
| where-true：`INSERT INTO public.upsert_doc AS dst VALUES (1,'candidate',30) ON CONFLICT (id) DO UPDATE SET v=excluded.v WHERE dst.v<excluded.v RETURNING id,email,v` | (1,'a',30)；1/INSERT 0 1；[(1,'a',30),(2,'b',20)]。 |
| where-false：`INSERT INTO public.upsert_doc AS dst VALUES (1,'candidate',3) ON CONFLICT (id) DO UPDATE SET v=excluded.v WHERE dst.v<excluded.v RETURNING id,email,v` | 零行；0/INSERT 0 0；B，不插入 candidate 行。 |
| where-null：`INSERT INTO public.upsert_doc AS dst VALUES (1,'candidate',3) ON CONFLICT (id) DO UPDATE SET v=excluded.v WHERE NULL::boolean RETURNING id,email,v` | 零行；0/INSERT 0 0；B。 |
| unchanged：`INSERT INTO public.upsert_doc AS dst VALUES (1,'candidate',3) ON CONFLICT (id) DO UPDATE SET v=dst.v RETURNING id,email,v` | (1,'a',10)；1/INSERT 0 1；B，区分跳过与实际无值变化更新。 |
| mixed：`INSERT INTO public.upsert_doc AS dst VALUES (1,'candidate',3),(3,'c',30) ON CONFLICT (id) DO UPDATE SET v=dst.v+excluded.v RETURNING id,email,v` | {(1,'a',13),(3,'c',30)}；2/INSERT 0 2；[(1,'a',13),(2,'b',20),(3,'c',30)]。 |

每例另建 `-no-returning` 变体，仅移除尾部 RETURNING id,email,v，最终行/标签相同且无结果集，以免返回门禁遮蔽冲突行为。simple 发 Q；extended 在 BEGIN 前 Parse s/Describe statement（无参数），BEGIN 后 Bind 新 portal p（空参数、text 结果）、Describe portal、Execute(max_rows=0)、Sync。Parse/Describe 前后 S 均 B。update 的参数化独立变体为 `INSERT INTO public.upsert_doc AS dst VALUES ($1,$2,$3) ON CONFLICT (id) DO UPDATE SET v=dst.v+excluded.v RETURNING id,email,v`，OID [23,25,23]、text 值 [1,'candidate',3]，结果同 update；重复 Execute 的副作用证据交协议生命周期，不能只计标签。

错误例每例重建 B，输入不带 RETURNING，须无成功行/标签；精确 SQLSTATE、S/V/C/M/D/H/P、阶段/原 SQL 字符位置均待参考端采集，不把类别名称写成实测结果：

- secondary-unique：`INSERT INTO public.upsert_doc VALUES (1,'b',3) ON CONFLICT (id) DO UPDATE SET email=excluded.email`，更新产生 email 唯一错误，不能改选 id=2 或忽略错误。
- check-update：`INSERT INTO public.upsert_doc VALUES (1,'candidate',3) ON CONFLICT (id) DO UPDATE SET v=-excluded.v`，合法候选在更新后违反 CHECK；not-null-update 独立输入 `INSERT INTO public.upsert_doc VALUES (1,'candidate',3) ON CONFLICT (id) DO UPDATE SET v=NULL`，必须失败。
- repeat-target：`INSERT INTO public.upsert_doc AS dst VALUES (1,'candidate',3),(1,'again',4) ON CONFLICT (id) DO UPDATE SET v=dst.v+excluded.v`，重复影响旧行须 cardinality violation，不能得到 17；repeat-new：`INSERT INTO public.upsert_doc AS dst VALUES (3,'c',3),(3,'d',4) ON CONFLICT (id) DO UPDATE SET v=dst.v+excluded.v`，本语句新插入行也不能被再次更新后成功。
- atomic：`INSERT INTO public.upsert_doc VALUES (3,'c',30),(1,'b',3) ON CONFLICT (id) DO UPDATE SET email=excluded.email`，第二行二次唯一错误须撤销第一行插入，最终 S 不能残留 id=3。
- other-key：`INSERT INTO public.upsert_doc VALUES (3,'b',30) ON CONFLICT (id) DO UPDATE SET v=excluded.v`，无 id 冲突但 email 冲突须报错。missing-target：`INSERT INTO public.upsert_doc VALUES (3,'c',30) ON CONFLICT DO UPDATE SET v=excluded.v`，缺少必要目标须拒绝；不能退化为普通插入。前者运行错误，后者诊断阶段独立取证。

idle 变体在 I 发 Q 或 Parse/Bind/Execute，错误后扩展先 Sync，Ready I，双方 S=B，SELECT 1 成功。transaction 变体 W BEGIN 后先 `INSERT INTO public.upsert_doc VALUES (9,'pending',90)` 再发错误输入；运行错误应发生于 Execute，Parse 拒绝不能计执行错误通过。O S 始终 B；错误后扩展 Sync 仍 E，W Q SELECT 1 目标 25P02/E，ROLLBACK 后 I、双方 S=B、SELECT 1 成功，pending 和部分写入均不残留。合法未开放形态另设 gate case：产品目标写前 0A000，参考端仍按成功目标取证，同样检查恢复和全值；当前正确门禁未验证。

两类 WHERE 独立夹具：事务外 `CREATE TABLE public.upsert_partial (id integer PRIMARY KEY,k integer,active boolean,v integer)`，`CREATE UNIQUE INDEX upsert_partial_active ON public.upsert_partial(k) WHERE active`，`INSERT INTO public.upsert_partial VALUES (1,7,true,10)`。W BEGIN 后发 `INSERT INTO public.upsert_partial AS dst VALUES (2,7,true,3) ON CONFLICT (k) WHERE active DO UPDATE SET v=dst.v+excluded.v WHERE dst.v<excluded.v RETURNING id,k,active,v`；目标谓词成功推断但动作条件 false，零返回/INSERT 0 0，W/O `SELECT id,k,active,v FROM public.upsert_partial ORDER BY id` 均 [(1,7,true,10)]。独立重建后仅将候选 3 改为 30，应返回 (1,7,true,40)，W 同值、O 仍初值；分别提交双方新值、回滚双方初值。未支持部分索引 setup 时记能力限制，不把 setup 拒绝算成功。

并发证据定义（待运行）：事务外以相同 DDL 独立重建空 upsert_doc（不插入 B），C1 BEGIN 后执行 `INSERT INTO public.upsert_doc VALUES (1,'a',10)` 不提交；C2 `BEGIN ISOLATION LEVEL READ COMMITTED`，发送 `INSERT INTO public.upsert_doc AS dst VALUES (1,'candidate',3) ON CONFLICT (id) DO UPDATE SET v=dst.v+excluded.v RETURNING id,email,v`，记录开始/等待及 C1 提交时序。C1 COMMIT 后 C2 目标 (1,'a',13)，C2 提交后双方 S=[(1,'a',13)]；独立重跑 C1 ROLLBACK 分支，C2 应插入 (1,'candidate',3)，提交后双方仅该行。需记录无重复行、无丢失更新、错误与重试、隔离设置及原始时序；再单设 where-false 持锁与条件重判、较高隔离错误/整事务重试用例，不在此推定结果或性能。AsterSQL 的等待、锁/唯一检查及隔离可用性均未验证，产品限制与 PG 参考结果分列。

MySQL 共存 `session.conflict-update.mysql.<query|prepared>.<commit|rollback>`，能力 SESSION-CONFLICT-UPDATE/mysql。M/N 同 Server 独立连接，事务外 `CREATE TABLE upsert_mysql_doc (id INT PRIMARY KEY,email VARCHAR(30) UNIQUE,v INT NOT NULL)`、`INSERT INTO upsert_mysql_doc VALUES (1,'a',10),(2,'b',20)`，记录 SET autocommit=1、sql_mode、隔离及 CLIENT_FOUND_ROWS。M BEGIN，Query `INSERT INTO upsert_mysql_doc VALUES (1,'candidate',3) ON DUPLICATE KEY UPDATE v=v+3`；prepared 变体独立重建，Prepare `INSERT INTO upsert_mysql_doc VALUES (?,?,?) ON DUPLICATE KEY UPDATE v=v+?`，整数/字符串/整数/整数值 [1,'candidate',3,3]。M `SELECT id,email,v FROM upsert_mysql_doc ORDER BY id` 应 [(1,'a',13),(2,'b',20)]，N 同查询仍 B；交错 PG 成功/失败恢复后重复查询及原 prepared SELECT `SELECT v FROM upsert_mysql_doc WHERE id=?`（1→13），元数据/警告对自身基线。ROLLBACK 后双方 B；独立 COMMIT 变体双方新值。此实际更新在 MySQL 8.4 的 affected rows 目标为 2，不能改成 PG 的 1；另独立 `INSERT INTO upsert_mysql_doc VALUES (1,'candidate',3) ON DUPLICATE KEY UPDATE v=v` 的未变值计数随 CLIENT_FOUND_ROWS 采集。MySQL 不存在 PG 目标/动作 WHERE 的等价保证，多唯一键不得删掉目标后直接换语法；精确诊断与本产品差异未实测。

全部清理先 ROLLBACK，Close 存在的 portal/statement/prepared，事务外分别 DROP TABLE public.upsert_doc、public.upsert_partial、upsert_mysql_doc，再关连接。来源步骤 1 映射行身份/更新条件/重复命中，步骤 4 映射 S、原子性/恢复及 MySQL 对照；本轮运行用例数 0。正确性风险为绑定、条件/基数检查与整语句原子性；兼容性风险为仲裁、隔离、拒绝阶段/诊断与返回计数；性能未测、无结论。

#### 只读公共表表达式契约

以下 `session.cte-read.pg.*` 均为**待新增、未运行**用例，关联 `SESSION-CTE-READ/pg`。PG W/O 同一独立测试库、UTF-8、UTC、search_path=public、初始 I，具备测试表权限。事务外逐条执行 `CREATE TABLE public.cte_read_doc (id integer PRIMARY KEY, v integer)` 与 `INSERT INTO public.cte_read_doc VALUES (1,10),(2,NULL)`；不存在真实 a/b/r 表。每组用前和用后 W/O 都执行副作用查询 S：`SELECT id,v FROM public.cte_read_doc ORDER BY id`，目标一直为 [(1,10),(2,NULL)]。成功 SQLSTATE 不适用；下列整数列固定 int4（OID 23/typlen 4/typmod -1）、text 格式，SQL NULL 单列记录，可空性与来源字段按参考采集，不由行值猜测。

| 用例后缀及完整查询输入 | 目标值/列身份与覆盖 | 失败恢复/副作用补证 |
| --- | --- | --- |
| `chain`：`WITH a(k,val) AS (SELECT id,v FROM public.cte_read_doc), b AS (SELECT k,val FROM a WHERE k=2) SELECT k,val FROM b ORDER BY k` | [(2,SQL NULL)]，列 k/val 均 int4；证明先前 CTE 依赖及 NULL 传递。把 k=2 改 k=3 为独立零行变体，列描述保持。 | S 不变；后续独立 `SELECT k FROM a` 应关系不存在，证明语句结束无名称泄漏；该反例独立采错误恢复。 |
| `repeat`：`WITH a AS (SELECT v FROM public.cte_read_doc UNION ALL SELECT v FROM public.cte_read_doc) SELECT l.v AS lv,r.v AS rv FROM a AS l CROSS JOIN a AS r ORDER BY lv NULLS LAST,rv NULLS LAST` | 共 16 行：(10,10)、(10,NULL)、(NULL,10)、(NULL,NULL) 各 4 行，列 lv/rv 均 int4；覆盖重复值、NULL 和独立消费者，不只核对行数。 | S 不变；不能由纯值结果证明物化策略或 volatile 调用次数。 |
| `shadow`：`WITH cte_read_doc AS (SELECT 7 AS id) SELECT c.id AS local_id,t.id AS table_id FROM cte_read_doc AS c CROSS JOIN public.cte_read_doc AS t ORDER BY table_id`；`nested`：`WITH a AS (SELECT 1 AS v) SELECT a.v AS outer_v,x.v AS inner_v FROM a CROSS JOIN (WITH a AS (SELECT 2 AS v) SELECT v FROM a) AS x` | 前者 [(7,1),(7,2)]，后者 [(1,2)]；四个结果列均 int4，名称按各投影，证明显式表绕过及内外层身份隔离。 | 两条分别运行，S 不变；后续语句可再次查询真实表，不延续 CTE。 |
| `partial-alias`：`WITH a(x) AS (SELECT id,v FROM public.cte_read_doc) SELECT x,v FROM a ORDER BY x` | [(1,10),(2,NULL)]，列 x/v 均 int4；别名列表仅重命名首列，不能因共享路径当前检查而改成失败目标。 | S 不变；现有源码全数量匹配检查只登记兼容性风险，当前 PG 实际分派及返回未验证。 |
| `name-errors`（各独立查询）：`WITH a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a`；`WITH a AS (SELECT 1 AS v) SELECT missing FROM a`；`WITH a AS (SELECT 1), a AS (SELECT 2) SELECT * FROM a` | 分别为不可见关系、缺列和同层重复名，目标 SQLSTATE 42P01 / 42703 / 42712；精确 S/V/M、D/H/P、位置及分析阶段待 PG 参考与产品采集，不从目录错误码推断应用错误。无成功行/成功标签。 | 各在 I 与 BEGIN 内独立运行下述失败恢复流程，逐次检查 S，不让前一次失败掩盖后项。 |
| `recursive-gate`：`WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM r WHERE n<3) SELECT n FROM r ORDER BY n` | PG 参考目标 [1,2,3]，列 n/int4；AsterSQL 当前限制目标 0A000，源码 scope 已有门禁但本轮未实测。 | 无写入；两入口分别取拒绝阶段、无成功行、S 及失败恢复。不要求参考 PG 报产品限制。 |

每条成功查询分别以简单 Q 与扩展 Parse/Bind/Describe/Execute/Sync 采集；零参数，text 结果。`chain` 再定义参数变体，把 `k=2` 换 `k=$1`，Parse 指定 OID [23]，新 Bind 值 2 和 3，分别应有 NULL 行/零行且类型不变。statement/portal Describe 遵守第 3 章，Describe 不执行原查询或样本查询，S 作为独立观察语句执行；执行结果、描述和 `SELECT n` 标签的实际 n 单列记录。重复 Execute 同一完成 portal 的行为沿工作流 7，不能重新求值成为额外查询成功证据。

失败恢复：I 中各错误后，扩展先 Sync 再 Q `SELECT 1 AS ok`，目标 I/一行 1；简单 Q 后直接该查询。BEGIN 变体 W 先 `BEGIN`、`INSERT INTO public.cte_read_doc VALUES (3,30)`，再执行该错误输入，目标 E；扩展 Sync 后仍 E，随后 Q `SELECT 1 AS ok` 目标 25P02/E。O 的 S 仍初始两行；W `ROLLBACK` 后 I，再 Q `SELECT 1 AS ok` 成功，W/O 的 S 都恢复初始两行，证明预先写入也回滚。错误字段沿本章公共模板，实际运行 observations 与 verdict 均为“未验证”。

MySQL 共存 `session.cte-read.mysql.coexist`（能力 `SESSION-CTE-READ/mysql`，独立 MySQL 契约）：同 Server 开 M/N、独立默认库、autocommit=1，记录 sql_mode/隔离级别；创建 `CREATE TABLE cte_mysql_doc (id INT PRIMARY KEY,v INT)`、`INSERT INTO cte_mysql_doc VALUES (1,10),(2,NULL)`。Query 与 Prepare/Execute 分别运行 `WITH a AS (SELECT id,v FROM cte_mysql_doc) SELECT id,v FROM a WHERE id=2 ORDER BY id`（目标 [(2,NULL)]）及参数版 `id=?`（整数 2、3 分别一行/零行）。M `BEGIN`、`INSERT INTO cte_mysql_doc VALUES (3,30)`，N 查询 `SELECT id,v FROM cte_mysql_doc ORDER BY id` 仍两行；交错 PG 拒绝/ROLLBACK 后 M `COMMIT`，N 应三行。PG 前后重跑 Query/新 Execute，MySQL 列类型/标志/NULL、code/SQLSTATE/警告各对自己的参考端与此前基线；不能要求其等于 PG OID 或让 MySQL 执行 public 限定/PG 物化语法作近似对照。当前所有 MySQL 结果同样未实测。

清理各变体先确保 W/M `ROLLBACK`，关闭 statements/portals，再在事务外分别 `DROP TABLE public.cte_read_doc`、`DROP TABLE cte_mysql_doc`，关所有连接；夹具每组重置，M 提交的第三行不串入其他组。人工覆盖映射：来源步骤 1 对应 chain/repeat/shadow/partial-alias 的真实应用查询，步骤 4 对应 name-errors/recursive-gate、S、回滚与 MySQL 共存；目录专例只作静态证据。正确性风险为重复引用消费、作用域及清理；兼容性风险为 PG 部分列别名、诊断和类型身份；性能与物化/递归资源未测试，不提供性能结论。

#### 写入公共表表达式契约

下列 `session.cte-write.pg.*` 均为**待新增、未运行**用例，关联 `SESSION-CTE-WRITE/pg`。PG W/O 连接同一独立测试库，UTF-8、UTC、search_path=public，初始 I，具备两表读写权限。事务外逐条创建 `CREATE TABLE public.cte_move_src (id integer PRIMARY KEY,v integer)`、`CREATE TABLE public.cte_move_dst (id integer PRIMARY KEY,v integer)`，无触发器、规则、序列或并发写者。每例独立重置 R：在 I 下逐条 `DELETE FROM public.cte_move_dst`、`DELETE FROM public.cte_move_src`、`INSERT INTO public.cte_move_src VALUES (1,10),(2,NULL),(3,30)`；O 在 autocommit/Read Committed 下每次新查询。副作用查询 S 为分别执行 `SELECT id,v FROM public.cte_move_src ORDER BY id` 和 `SELECT id,v FROM public.cte_move_dst ORDER BY id`；初始结果分别 [(1,10),(2,SQL NULL),(3,30)] 与 []。

完整搬移输入 Q：`WITH moved AS (DELETE FROM public.cte_move_src WHERE id<=2 RETURNING id,v) INSERT INTO public.cte_move_dst(id,v) SELECT id,v FROM moved RETURNING id,v`。整数输出列 id/v 固定 int4（23/4/-1），text 格式，NULL 编码与元数据复用返回块；多行按含重复项的多重集比较，RETURNING 无隐含排序。每例记录值/NULL、列序/名称、OID/typlen/typmod、来源字段、标签/影响行数及 I/T/E；成功 SQLSTATE 不适用，错误还须采集 severity/message/position/detail/constraint 等适用字段，尚无精确参考采集结果。

| 用例 / 完整输入或 Q 的明确变体 | 目标结果、快照与副作用证据 | 恢复/继续性 |
| --- | --- | --- |
| `move`：R 后 W 执行 Q | 返回 {(1,10),(2,SQL NULL)}，主标签 INSERT 0 2；W/O 的 S 都为源 [(3,30)]、目的 [(1,10),(2,SQL NULL)]，不能只比总行数。 | 成功 I；W `SELECT 1` 成功，再 S；结束 R。 |
| `empty`：R 后 Q 的 `id<=2` 改为 `id<0` | 零行且同样两列描述，INSERT 0 0；W/O 的 S 均保持初始值。 | I；`SELECT 1` 后 R。 |
| `rollback` / `commit`：R，W `BEGIN`，执行 Q，W/O 分别 S，再分别 `ROLLBACK` 或 `COMMIT`（两次独立试验） | Q 返回两行；提交前 W 见搬移后值、O 仍初始值。ROLLBACK 后双方均初始；COMMIT 后双方均搬移后值，两个表都采集。 | T→I 后 `SELECT 1` 和 S，结束 R；不以返回成功代替回滚证明。 |
| `destination-fail`：R 后先 `INSERT INTO public.cte_move_dst VALUES (2,200)`，再 Q；隐式及显式 `BEGIN` 各独立一次，后者在 Q 前再 `INSERT INTO public.cte_move_src VALUES (9,90)` | 主 INSERT 唯一键失败，目标 23505；O 的 S 始终源初始、目的 [(2,200)]，不允许源丢 1/2 或目的多 1。显式失败期间不在 W 用 S 绕过门禁；回滚后双方 S 也须相同且无 9。输入先写 9 用来证明整个事务恢复。 | 隐式回 I，`SELECT 1` 成功；显式 E，`SELECT 1` 目标 25P02，Sync 后仍 E，ROLLBACK→I 后再 S/SELECT 1；结束 R。 |
| `source-fail`：R；Q 的 CTE 返回列表改为 `RETURNING id,10/(id-2) AS v`，外层不变 | 源侧投影在 id=2 除零，目标 22012；不得假设扫描先后，W/O 的 S 最终均初始，既无源部分删除也无目的部分插入。不把可能提前收到的返回行当作成功。 | 隐式失败回 I 后 S/SELECT 1；另用 BEGIN 重跑，E、Sync、ROLLBACK→I 后 S；结束 R。 |
| `snapshot`：R；`WITH moved AS (DELETE FROM public.cte_move_src WHERE id<=2 RETURNING id,v), seen AS (SELECT id,v FROM public.cte_move_src WHERE id<=2) INSERT INTO public.cte_move_dst SELECT seen.id,seen.v FROM seen JOIN moved ON seen.id=moved.id RETURNING id,v` | 返回两行且 S 为搬移后值；seen 必须读到被删前两行，moved 必须提供被删行。与 Q 对照，不能用执行顺序令 seen 变零行。此例仅证明这组可见性，不能覆盖所有隔离级别。 | I；S/SELECT 1 后 R。 |
| `repeat-consumer`：R；`WITH moved AS (DELETE FROM public.cte_move_src WHERE id<=2 RETURNING id,v) INSERT INTO public.cte_move_dst SELECT a.id,a.v FROM moved AS a JOIN moved AS b ON a.id=b.id RETURNING id,v` | 返回两行、S 为搬移后值；第二引用不能重复 DELETE 或耗尽首引用行集。 | I；S/SELECT 1 后 R。 |
| `unused` / `unreturned`（后续独立开放门槛）：R；分别 `WITH moved AS (DELETE FROM public.cte_move_src WHERE id<=2 RETURNING id,v) SELECT 7 AS marker` 与 `WITH moved AS (DELETE FROM public.cte_move_src WHERE id<=2) INSERT INTO public.cte_move_dst VALUES (4,40)` | PG 目标前者 [(7)]（int4），S 源 [(3,30)]、目的 []，即使无人读也完成 DELETE；后者主 INSERT 影响一行，S 源 [(3,30)]、目的 [(4,40)]。未开放时 AsterSQL 以整体能力拒绝验收，不能跳过 DELETE 后成功。 | 两项分别 R；成功 I，否则依下行拒绝恢复；各自 S/SELECT 1，结束 R。 |

当前限制拒绝 `session.cte-write.pg.reject` 用非空夹具 R 的原始 Q 验证整体门禁，并用 `SELECT * FROM (WITH moved AS (DELETE FROM public.cte_move_src WHERE id<=2 RETURNING id,v) SELECT * FROM moved) AS x` 验证非顶层写入反例（PG 本身拒绝，精确 SQLSTATE/位置待参考端采集，不套用产品 0A000）。逐项在隐式与 BEGIN 场景运行，任一拒绝均要求 O 的 S 初始；隐式错误后 I、SELECT 1 成功，显式错误后 E、Sync 不恢复、ROLLBACK→I 后 W/O 的 S 初始。递归门禁输入另用 `WITH RECURSIVE keys AS (SELECT 1 AS id) DELETE FROM public.cte_move_src WHERE id IN (SELECT id FROM keys) RETURNING id,v`：PG 合法目标删除 (1,10)，AsterSQL 当前限制目标 0A000 且 S 不变；直接递归写入的非法结构和诊断另采，不伪造结果。各反例结束 R，最后在 I 下逐条 DROP 两表；不在显式事务内用 DDL 恢复夹具。

Query 与扩展 Parse/Bind/Describe/Execute 分别覆盖 Q、零行及成对失败。参数版把 `id<=2` 换 `id<=$1`、OID 23/text 值 2 和 0 分两次新 Bind/Execute；Parse/Describe 前后 S 必须相同，元数据不取样写入。一次 portal 分段/重复 Execute 只发送该次结果、不重做搬移；新 Bind 的新语句执行须重新求值。扩展运行错误采集 ErrorResponse、Sync 后事务状态及上述恢复；尚未支持时记录拒绝阶段和无副作用，不能算成功搬移证据。快照门槛还须由共享执行层的独立测试/可观测执行记录证明各分支共用快照与一次生产；仅行值相同不证明调用次数或全部并发语义。

MySQL 共存 `session.cte-write.mysql.coexist`（`SESSION-CTE-WRITE/mysql`，独立契约）：同 Server 的 M/N、独立默认库、autocommit=1，记录 sql_mode/隔离级别。逐条 `CREATE TABLE cte_move_mysql (id INT PRIMARY KEY,v INT)`、`INSERT INTO cte_move_mysql VALUES (1,10),(2,NULL)`；M 以 Query 执行 `SELECT id,v FROM cte_move_mysql ORDER BY id`，目标 [(1,10),(2,SQL NULL)]；Prepare/Execute 执行 `SELECT id,v FROM cte_move_mysql WHERE id=? ORDER BY id`（整数 2），目标 [(2,SQL NULL)]。M `BEGIN`、`UPDATE cte_move_mysql SET v=11 WHERE id=1`，N 新查询 `SELECT id,v FROM cte_move_mysql ORDER BY id` 仍 [(1,10),(2,NULL)]；交错 PG Q 成功或拒绝/恢复后 M `COMMIT`，N 见 [(1,11),(2,NULL)]。另从初值单独做同一 UPDATE 后 ROLLBACK，N 仍初值。PG 前后再查 Query/新 Execute，比较 MySQL 自己的值、类型/标志、code/SQLSTATE/警告与参考/此前基线，末尾 DROP 表。此例不让 MySQL 执行 PG 写入 CTE，不用两条 MySQL DML 模拟 PG 单语句原子性；所有结果未实测。

#### 模式与搜索路径契约

以下全部为**待新增、未运行**用例，关联 `SESSION-SEARCH-PATH/pg`，各行及变体独立 case_id `session.search-path.pg.<名称>.<query|extended>`。PG A/B 同一独立测试库、UTF-8、UTC、初始 I，无临时对象；参考端具备 public 的 USAGE/CREATE/SELECT/UPDATE 权限，将该夹具的默认路径配置为 public，另记录未覆盖配置时的真实默认。事务外逐条准备 `CREATE TABLE public.path_doc (id integer PRIMARY KEY,v integer)`、`INSERT INTO public.path_doc VALUES (1,10)`、`CREATE TABLE public.pg_namespace (nspname text)`、`INSERT INTO public.pg_namespace VALUES ('user_marker')`，没有 missing_t、missing schema 或 private schema。若 AsterSQL 在准备阶段拒绝，记录门槛失败，不将正文预期当运行结果。

下表缩写均为确定 SQL：SHOW=`SHOW search_path`，RESET=`RESET search_path`，SET public/pg_catalog 分别为 `SET search_path TO public` / `SET search_path TO pg_catalog`，SET LOCAL public/pg_catalog 分别为 `SET LOCAL search_path TO public` / `SET LOCAL search_path TO pg_catalog`；未限定 SELECT 默认指 `SELECT id,v FROM path_doc ORDER BY id`（明确写 id 的行使用该行 SQL）。BEGIN/COMMIT/ROLLBACK 各独立发送，所有变体重新建立上述初始状态。每例开始 A/B 各 `SET search_path TO public`；副作用查询 S 是双方在允许执行时各 `SELECT id,v FROM public.path_doc ORDER BY id`，基线 [(1,10)]。目录身份查询 C 为 `SELECT nspname FROM pg_catalog.pg_namespace WHERE nspname = 'pg_catalog'`，目标一行 pg_catalog；这里只比较稳定标记，不伪造 OID。清理先在 A/B 各 ROLLBACK、RESET search_path，再在 I 下逐条 DROP TABLE public.pg_namespace、DROP TABLE public.path_doc、关闭连接；若准备部分失败只删除实际创建的对象，不遗留路径/事务状态。

| 名称与完整顺序输入（每条独立发送） | 目标结果、副作用和恢复 |
| --- | --- |
| isolation：A `SET search_path = pg_catalog`；B `SHOW search_path`、`SELECT id,v FROM path_doc ORDER BY id`；A `SHOW search_path`、`SELECT id,v FROM path_doc ORDER BY id`；A `SELECT id,v FROM public.path_doc ORDER BY id`；B 重复自身查询；A `RESET search_path`、`SHOW search_path`、未限定 SELECT | B 始终 public/[(1,10)]，A SHOW 为 pg_catalog，未限定缺表目标 42P01；I 中失败后显式 public 立即成功，RESET 后 public/未限定读取恢复。S 前后不变，另一连接无模式或库变化。 |
| order：A 依次 SET 为 public、pg_catalog, public、public, pg_catalog，每次执行 `SELECT nspname FROM pg_namespace WHERE nspname = 'pg_catalog'`、C、`SELECT nspname FROM public.pg_namespace ORDER BY nspname`；每轮 B 同隐式查询及 SHOW | A 隐式前两轮一行 pg_catalog，第三轮零行；显式 C 始终一行，public 显式始终 user_marker。B 的 public 原值及隐式 catalog 选择不变；S 不变。证明首匹配对象选择，不能将两表合并模拟。 |
| empty：A `SET search_path TO ''`、SHOW、`SELECT current_schema()`、C、`SELECT id FROM path_doc`、`SELECT id FROM public.path_doc`；B SHOW/未限定 SELECT；A RESET 并重查 | A 原值空串/schema SQL NULL，C 仍一行，未限定表目标 42P01，显式返回 1；B 始终 public/1；I 失败后可继续，RESET 后恢复未限定读取。S 不变。 |
| missing/gate：各自独立 I，A `SELECT id FROM public.missing_t`；另例 A `SET search_path TO missing, public`、SHOW、未限定 SELECT；另例 A `SELECT id FROM private.path_doc`；每次 B SHOW/S | 缺表目标 42P01；完整 PG 目标接受 missing 设置并忽略无效项、仍读 1，当前有界门禁预期 0A000 且原路径不变，须记录为限制。private 在此不存在，PG 诊断从参考采集；当前非 public 门禁 0A000 与 PG 不等价。各 I 拒绝后 SHOW/SELECT 可继续，S 无变更；语法错另例 `SET search_path TO` 的位置/码实际采集。 |
| commit/rollback：A SET public；BEGIN；SET pg_catalog；SHOW；COMMIT；SHOW；显式 S；再 SET public；BEGIN；SET pg_catalog；SHOW；ROLLBACK；SHOW；未限定 SELECT；各转换间 B SHOW/未限定 SELECT | 第一次提交后 pg_catalog 持续；第二次回滚恢复 public，未限定返回 [(1,10)]。两次事务中 B 均 public，S 不变。RESET 变体独立从 SET pg_catalog 开始，BEGIN、RESET、SHOW（夹具默认 public）、ROLLBACK、SHOW（pg_catalog）；另跑 COMMIT 变体结束为 public。 |
| failed：A SET public；BEGIN；SET pg_catalog；`UPDATE public.path_doc SET v=11 WHERE id=1`；`SELECT id FROM public.missing_t`；依次尝试 SHOW、SET public、RESET；扩展模式发送 Sync 后再尝试 SHOW；ROLLBACK；SHOW；未限定 SELECT；B 在各阶段查 S/SHOW | 首错进入 E，后续设置/读取须被失败门禁拒绝（目标 25P02，原始字段待采），Sync 后仍 E；ROLLBACK 后 I/public/[(1,10)]。B 始终 public/[(1,10)]。E 中不运行会污染状态的 S，恢复后用 S 证明 v=11 未残留。 |
| local：独立 A SET public；BEGIN；SET LOCAL search_path TO pg_catalog；SHOW；COMMIT 或 ROLLBACK；SHOW/未限定 SELECT；另例 BEGIN、SET pg_catalog、SET LOCAL public、SHOW、COMMIT、SHOW；再做同顺序 ROLLBACK 变体；另在 I 直接 SET LOCAL pg_catalog、SHOW；每步 B SHOW/S | 目标第一组事务内 pg_catalog、结束均 public；混合组事务内 public、提交后 pg_catalog、回滚后事务前 public（每个变体重置）；I 下 LOCAL 警告且仍 public。当前不能声明支持：若能力门禁触发，须先记拒绝、协议同步及 ROLLBACK，再查双方路径/S 不变，判为门禁证据而非成功目标通过。 |

Q 与扩展 Parse/Bind/Describe/Execute 两组逐条取证；扩展每条采用新 statement/portal、无参数、全量 Execute 后 Sync，避免把缓存问题混入本题，失败后 Sync 再恢复。记录 SHOW 一列 search_path 的空串/非空串、current_schema 的 SQL NULL 与列身份、SELECT 整数/文本结果、列名、OID/typlen/typmod/格式/可空性、命令标签、错误原文/SQLSTATE/位置、警告及 I/T/E 序列，精确 wire 字段均待采，不能由原生 metadata 猜测。成功 SQLSTATE 不适用，SET/RESET 无结果列；失败阶段可在 Parse 或执行，保留实测阶段。

MySQL 共存 `session.search-path.mysql.<query|prepared>` 为 `SESSION-SEARCH-PATH/mysql` 的自身入口保留契约，与上表关联。M/N 同 Server 独立当前库，`SET autocommit=1` 并记录 sql_mode/隔离级别；事务外 `CREATE TABLE path_mysql_doc (id INT PRIMARY KEY,v INT)`、`INSERT INTO path_mysql_doc VALUES (1,10)`。保存 Query `SELECT DATABASE()`、`SELECT id,v FROM path_mysql_doc ORDER BY id` 与 Prepare `SELECT v FROM path_mysql_doc WHERE id=?`/Execute 整数 1 的元数据和值；M BEGIN、`UPDATE path_mysql_doc SET v=11 WHERE id=1`，M 查询 11/N 查询 10，交错 PG A/B 的路径切换及失败恢复后重复原 Query/prepared 与模式/库检查，M ROLLBACK 后 M/N 均 [(1,10)]。值、类型标志/charset/length/decimals、警告及错误均对自身 Go/TiDB/MySQL 参考，不向 MySQL 发送 PG SET 来推断等价。清理 M ROLLBACK、关闭 prepared、事务外 DROP TABLE path_mysql_doc、关闭 M/N。

当前各端原始 observations 未采集、verdict 未验证、有效运行数 0；文档验收仅确认输入、目标/门禁、副作用与恢复完整。目录展示与真实对象解析分开，身份/权限依赖工作流 6，18.6 精确兼容仍待实证。

#### 引号与大小写执行契约

以下全部是**待新增、未运行**用例，`session.name-case.pg.<名称>.<query|extended>` 关联 `SESSION-NAME-CASE/pg`。PG W/O 同一全新独立测试库，UTF-8、UTC、search_path=public、初始 I，具备夹具权限，无临时对象。事务外逐条建立 `CREATE TABLE public.t (id integer PRIMARY KEY,c integer,"C" integer)`、`INSERT INTO public.t VALUES (1,10,11)`、`CREATE TABLE public."T" (id integer PRIMARY KEY,c integer)`、`INSERT INTO public."T" VALUES (2,20)`、`CREATE TABLE public."a.b" (id integer PRIMARY KEY)`、`INSERT INTO public."a.b" VALUES (3)`。若任一创建因大小写冲突或表示能力拒绝，记录准备阶段门槛失败及未建对象，不伪称成功覆盖。只含 public、没有大写 PUBLIC schema；另有已存在独立数据库 other_db，访问拒绝用例不得读取它。

副作用查询 S 为 W/O 各逐条 `SELECT id,c,"C" FROM public.t ORDER BY id`、`SELECT id,c FROM public."T" ORDER BY id`、`SELECT id FROM public."a.b" ORDER BY id`，基线分别 [(1,10,11)]、[(2,20)]、[(3)]。每例独立重置夹具，开始/恢复后双方 S，禁止靠目录名称或改写 SQL 通过。所有命令逐条发送；Q 与扩展两变体均用无参数、新建 statement/portal，Describe 不执行写入，Execute 取完后 Sync，错误后先 Sync 再按事务状态恢复。

| 名称与完整输入 | 目标值、对象及拒绝/恢复 |
| --- | --- |
| relation：分别 `SELECT id FROM t`、`SELECT id FROM T`、`SELECT id FROM "t"`、`SELECT id FROM public.t`、`SELECT id FROM PUBLIC.T`、`SELECT id FROM "public"."t"`；另执行 `SELECT id FROM "T"`、`SELECT id FROM public."T"` | 前六条均只返回 1，后两条只返回 2；S 不变。另建仅 public.t 的独立夹具（同 t 定义/初值，不建其余两表），`SELECT id FROM "T"` 必须缺表，不能回落为 1；I 下失败后 `SELECT id FROM t` 立即返回 1，随后清理该夹具。 |
| column：`SELECT c,C,"c","C" FROM public.t`；独立负例 `SELECT "cC" FROM public.t` | 正例一行 (10,10,10,11)，列名依次 c,c,c,C；负例缺列，I 下下一条 `SELECT c FROM public.t` 返回 10。S 不变，不能把全部列统一到 c。 |
| alias：`SELECT A.c FROM public.t AS A`；`SELECT "A".c AS "V" FROM public.t AS "A"`；反例 `SELECT a.c FROM public.t AS "A"`、`SELECT t.c FROM public.t AS "A"`；输出别名对照 `SELECT c AS V,c AS "V" FROM public.t` | 前两条均返回 10，第二条列名 V；反例不可回落原表/别名，应名称绑定失败，精确码/位置待采。最后一条 (10,10)，列名 v,V。每个 I 反例后重跑第一条成功，S 不变。 |
| dot/literal：`SELECT id FROM public."a.b"`；`SELECT 'public.T', '"T"', 'a.b' FROM public.t /* public."T" */`；`SELECT id FROM other_db.public.t`；`SELECT id FROM "PUBLIC".t` | 首条 3；第二条一行原字符串 public.T、"T"、a.b，不能命中大写表。跨库拒绝，当前产品门禁 0A000；引用 PUBLIC 在本夹具不存在，PG 诊断待采，当前有界 schema 门禁 0A000 与 PG 不等价。I 中每个错误后首条仍返回 3，S 不变。当前库三段另将 W startup 的实际数据库名按 PG 标识符引号转义填入 `SELECT id FROM <当前库>.public.t`，期望 1；记录替换后的完整输入，不能发送占位符。 |
| write/rollback：W `BEGIN`、`UPDATE public."T" SET c=21 WHERE id=2`、S、`ROLLBACK`、S；独立 COMMIT 变体重建夹具重跑，将 ROLLBACK 替换为 COMMIT | 写后 W 仅大写表为 (2,21)，小写表/点号表不变；O 提交前始终基线。ROLLBACK 后双方全基线；COMMIT 后双方仅大写表为 (2,21)，证明真实对象写入及持久可见性。 |
| failed：W `BEGIN`、`UPDATE public."T" SET c=21 WHERE id=2`、`SELECT "cC" FROM public.t`、`SELECT c FROM public."T"`、Sync（仅扩展）、`ROLLBACK`、S；O 在各阶段 S | 首错后 E，后续 SELECT 被失败门禁拒绝，Sync 后仍 E；ROLLBACK 后 I/全基线，O 始终基线。首错缺列及后续失败门禁的精确字段、阶段与参考码待采，不将本轮零运行写成通过。 |

另在各独立 I 夹具执行词法负例 `SELECT id FROM public.""`、`SELECT id FROM public."broken`，拒绝后 `SELECT id FROM public.t` 应仍返回 1，S 不变；空/未闭合引号不能进入写入。成功 SQLSTATE 不适用。各例记录行值/NULL、列顺序与名字、OID/typlen/typmod、来源字段/格式、标签/影响行数、错误 SQLSTATE/message/position 及 I/T/E；整数目标 int4（23/4/-1）、text 格式，字符串按第 3 章 text 身份采证，其余精确字段不猜测。错误定位复用第 2 章原查询字符跨度规则。预期与原始 observations 分列，当前 observations 未采集、verdict 未验证、有效运行用例数 0。

MySQL 共存 `session.name-case.mysql.<query|prepared>` 关联 `SESSION-NAME-CASE/mysql`：M/N 同 Server 的独立测试库、autocommit=1，保存 `SELECT DATABASE()`、`SELECT @@sql_mode`、`SELECT @@lower_case_table_names` 及平台/版本。事务外逐条 `CREATE TABLE name_case_mysql (id INT PRIMARY KEY,c INT)`、`INSERT INTO name_case_mysql VALUES (1,30)`，保存 Query `SELECT c,C FROM name_case_mysql` 的 (30,30) 与 Prepare `SELECT c FROM name_case_mysql WHERE id=?`/Execute 整数 1 的 30 及元数据。M BEGIN、`UPDATE name_case_mysql SET c=31 WHERE id=1`，M/N 查询分别 31/30；交错全部 PG 读取、写入和拒绝恢复后，重复 MySQL Query/prepared、库/模式/配置查询，结果按自身事务持续，M ROLLBACK 后双方 30。双引号模式独立变体先在 M 保存 `SET @name_case_saved_mode=@@SESSION.sql_mode`，分别执行 `SET SESSION sql_mode=''` 与 `SET SESSION sql_mode='ANSI_QUOTES'`（每次均交错 PG 操作及检查 N），各模式下执行 `SELECT "c" FROM name_case_mysql`，目标分别字符串 c 与列值 30（该变体从回滚后的初态开始）；每次变体后 `SET SESSION sql_mode=@name_case_saved_mode` 恢复并核对原始模式，N 不变。采集 MySQL 自身列类型/flags/charset、警告和错误，不套 PG 精确名称规则；准备/配置不可用就登记门槛，不能改全局设置使其通过。

清理先各连接 ROLLBACK、关闭存在的 statements/portals/prepared，恢复 M 原 sql_mode；再在 I 下逐条 DROP TABLE public."a.b"、DROP TABLE public."T"、DROP TABLE public.t，MySQL DROP TABLE name_case_mysql，关闭连接；只清理实际建立的对象。来源步骤 2 → relation/column/alias/dot，步骤 4 → S、write/failed 与 MySQL 共存。正确性风险为误绑定/合并和错误前副作用，兼容性风险为后端大小写配置、列名及诊断差异，性能未测、无结论。

#### 预处理执行时会话契约

以下 `session.prepared-state.pg.<名称>` 全为**待新增、未运行**用例，关联 `SESSION-PREPARED-STATE/pg`。PG A/B/O 同一全新独立测试库、UTF-8、UTC、Read Committed、初始 I，有夹具权限，无其他并发写者。每例独立初始化，事务外逐条 `CREATE TABLE public.prep_doc (id integer PRIMARY KEY,v integer)`、`INSERT INTO public.prep_doc VALUES (1,10),(2,20)`；三连接 `SET search_path TO public`。S 为双方在可执行时 `SELECT id,v FROM public.prep_doc ORDER BY id`，初始 [(1,10),(2,20)]。改变对象的例以指定 DDL 后状态为基线，拒绝不得再修改它。

P 表示 A 命名 Parse s、SQL `SELECT v FROM prep_doc WHERE id=$1`、OID [23]、Describe statement、Sync 至 I；E(x) 表示新 Bind portal p 到 s（text 参数 x、text 结果）、Describe portal、Execute(max_rows=0)、Sync 至 I，p 随事务结束销毁。每次改变环境前用 Sync 释放隐式事务/解析锁，另连接 DDL 必须等待完成再继续，不在活跃旧 portal 上强行 DROP。每次 E 前都采 Describe；参数恒 int4/OID23，基线一列 v 为 int4/23/typlen4/typmod -1，结果 x=1 为10、x=2为20，命令标签 SELECT 1。列名、顺序、来源表 OID/属性号、格式及可空性分别采证；不伪造 OID 数值。表内 NULL 不参与此夹具，参数 NULL 的独立 E(NULL) 应零行且类型不变。

| 名称与完整环境变化/输入 | 目标对象、结果、元数据/拒绝及副作用 |
| --- | --- |
| stable：P；E(1)、E(2)、E(NULL)；B 同名 Parse s 为 `SELECT id FROM public.prep_doc WHERE id=$1`/OID23，再以自己新 portal 绑定1执行；A 再 E(1) | A 为10/20/零行/10，B 为1且列名 id、int4，statement/portal名字相同也互不引用。A/B Describe 各自稳定；S 不变，参数值不改列身份。 |
| path-missing：P、E(1)；A `SET search_path TO pg_catalog`；E(1)；恢复 `SET search_path TO public` 后 E(1)；B S/SHOW | 变更后新使用须重新分析、缺 prep_doc 拒绝（缺关系目标42P01，精确阶段/报文待采）；恢复后10、v/int4。独立 qualified 变体 P 的 SQL 仅改 `FROM public.prep_doc`，路径切换后仍10。B 路径 public，S 不变。 |
| path-switch：另建 `CREATE SCHEMA prep_alt`、`CREATE TABLE prep_alt.prep_doc (id integer PRIMARY KEY,v integer)`、`INSERT INTO prep_alt.prep_doc VALUES (1,90)`；P、E(1)；A `SET search_path TO prep_alt, public`；E(1)；`SET search_path TO public` 后再 E(1) | 对象 public→prep_alt→public，值10→90→10，v/int4不变、来源身份另采；S 仍初值，另查 `SELECT id,v FROM prep_alt.prep_doc ORDER BY id` 为[(1,90)]。产品当前非 public 门禁须记准备/设置限制及路径未变，不能计重绑定通过。 |
| earlier-shadow：独立上述两 schema，但起初只有 public.prep_doc；A `SET search_path TO prep_alt, public`；P、E(1)；B 创建 prep_alt.prep_doc/插入(1,90)；A E(1)；再 A `SET search_path TO public`、E(1)、`SET search_path TO prep_alt, public`、E(1) | 仅创建早位同名表不自动切换，第一次仍10；显式路径变化触发重新分析后最终90。两表 S 各保持初值/90，v/int4稳定，来源身份随有效重绑定采证。该目标同样受多 schema 准备门槛限制。 |
| drop-recreate：P、E(1)；B `DROP TABLE public.prep_doc`；A E(1)失败；B 逐条 `CREATE TABLE public.prep_doc (id integer PRIMARY KEY,v integer)`、`INSERT INTO public.prep_doc VALUES (1,70),(2,80)`；A E(1) | 缺对象失败且无数据输出；同结构重建后70、v/int4，必须对照新目录对象身份，不能用旧对象行或旧 portal 缓存代替。双方 S 最终[(1,70),(2,80)]，失败不额外写入；DDL 都已提交，不混入事务型 DDL。 |
| ddl-shape：独立 P 的 SQL 改为 `SELECT * FROM public.prep_doc WHERE id=$1`，基线列[id,v]/int4,int4；E(1)；B `ALTER TABLE public.prep_doc ADD COLUMN extra integer`；A E(1) | 旧 statement 固定结果两列，重新分析出现三列应拒绝，精确错误字段/阶段待采。Close s、新 Parse 同SQL/OID23、Describe、E(1)应(1,10,NULL)，列[id,v,extra]/三int4；另查 `SELECT id,v,extra FROM public.prep_doc ORDER BY id` 为[(1,10,NULL),(2,20,NULL)]。独立显式投影 P 原SQL对照新增无关列仍10/v/int4；新增列本身不必让所有 statement 失败。 |
| bound-path：P；A Q `BEGIN`；Bind p 值1、Describe portal；A Q `SET search_path TO pg_catalog`；Execute p、Sync；ROLLBACK；A E(1) | p 已绑定 public 对象，应10且v/int4；不能因后来的路径切换把已有portal当新Bind重新解析。Sync在显式事务中仍T；回滚/恢复public后新E仍10，S不变。相同独立 current_schema 变体：Parse `SELECT current_schema()`/无参数；SET后新Bind/Describe/Execute应pg_catalog，结果name类型身份按第3章/参考端采集，不能套产品当前text证明等价。 |
| catalog-live：P的SQL改 `SELECT relname FROM pg_catalog.pg_class WHERE relname='prep_later'`/无参数；Describe、Sync，首轮新Bind/Execute零行；B `CREATE TABLE public.prep_later (id integer)`；A再新Bind/Describe/Execute；B `DROP TABLE public.prep_later`；A再新Bind/Execute | 目录行零→prep_later→零，列relname的name身份/元数据各阶段保持（产品当前目录映射另列差异），S不变。只证明目录新执行读取已提交变化；分段或重复Execute同portal不重新取快照，不能借此证明普通s失效。 |

无参数用例的每轮均 Bind 新 portal（空参数、text结果）、Describe portal、Execute(max_rows=0)、Sync至I，才能在B执行下一条DDL。表中SHOW为 `SHOW search_path`；S在A/B/O能执行时以独立Q采集，不能在Bind/Execute之间插入会销毁未命名对象的查询。E(NULL)标签为SELECT 0，其他成功行数按实际结果记录，成功SQLSTATE不适用。

上述失败在 I 下记录 ErrorResponse、Sync→I，再 `SELECT 1 AS ok` 及新合法 Bind/Execute应可继续。另以 path-missing 的合法 P 在 BEGIN 前准备、环境 pg_catalog 在 BEGIN 前设置；A BEGIN、`INSERT INTO public.prep_doc VALUES (9,99)`、新 Bind/Describe/Execute s 触发缺表失败，Sync 应 E，Q SELECT 1 目标25P02，ROLLBACK→I；SET public 后 E(1)恢复10，A/B/O 的 S 均原基线、无9。记录失败究竟发生于 Bind/Describe/Execute，不把 Parse 提前拒绝算作此用例通过。各读取用例 Parse/Describe 前后 S 相同，输出失败不能隐含已执行写入。拒绝不提供成功列数据；原描述、错误SQLSTATE/原文/P字段有无与位置、I/T/E和恢复后列描述逐项留原始证据。

连接隔离负例另从 stable 结束 I 开始：B先Close自己的s并Sync；A保留s；B Bind到s必须失败，Sync后 `SELECT 1 AS ok` 继续。A BEGIN、Bind仅自己命名的p_a（s、值1），B Execute p_a也必须拒绝，B Sync/SELECT1恢复，A Execute p_a仍返回10/v/int4，A Sync后ROLLBACK。再B重新Parse s（stable中B的id查询/OID23）、Describe/Sync；A/B各BEGIN、各Bind同名p（自身s、值1）、Describe，A Close自己的s，B Execute p仍返回id=1/int4，A新Bind s拒绝并Sync→E；B Sync→T，分别ROLLBACK→I，再重建各自s。跨连接不得凭数值相同省略各自Describe/所有权证据；命名statement持续至Close/断连，portal仅当前事务有效，跨Sync/事务结束重用及未命名替换的精确序列交工作流7。

MySQL共存 `session.prepared-state.mysql.<query|prepared>` 关联 `SESSION-PREPARED-STATE/mysql`：M/N同Server独立测试库，autocommit=1，记录sql_mode/隔离。事务外 `CREATE TABLE prep_mysql (id INT PRIMARY KEY,v INT)`、`INSERT INTO prep_mysql VALUES (1,30)`；保存Query `SELECT v FROM prep_mysql WHERE id=1`、MySQL Prepare `SELECT v FROM prep_mysql WHERE id=?`/Execute整数1的30及自身列类型/flags/charset/length/decimals。M BEGIN、`UPDATE prep_mysql SET v=31 WHERE id=1`，M两入口31/N Query30；交错PG路径切换、跨连接DDL、拒绝恢复与Close，重复原MySQL Query/prepared、`SELECT DATABASE()`和`SELECT @@sql_mode`应保持自身基线与事务可见性；M ROLLBACK后两连接30。PG search_path与固定类型诊断不套用到MySQL，MySQL自身DDL自动重准备策略另证。

清理先全部ROLLBACK，恢复PG search_path public，关闭实际存在的portal/statement/MySQL prepared；事务外按实际创建情况逐条 `DROP TABLE public.prep_later`、`DROP TABLE prep_alt.prep_doc`、`DROP SCHEMA prep_alt`、`DROP TABLE public.prep_doc`，MySQL `DROP TABLE prep_mysql`，再关闭连接。来源步骤2映射四时点、path/drop/ddl/cross-connection，步骤4映射S、失败恢复/MySQL；本轮运行用例数0，无法从结构检查提升支持等级。

#### 序列执行契约

下列 `session.sequence.pg.*` 均为**待新增、未运行**，关联 `SESSION-SEQUENCE/pg`。A/B 为同库两条全新 PG 连接，UTF-8、UTC、search_path=public、autocommit、初始 I；管理员具备全部夹具权限。每例独立用事务外 DDL 创建 `CREATE SEQUENCE public.seq_contract AS bigint INCREMENT BY 1 MINVALUE 1 MAXVALUE 1000 START WITH 10 CACHE 1 NO CYCLE`，并创建空表 `CREATE TABLE public.seq_rows (id integer PRIMARY KEY,v bigint)`；不设置默认值、owned-by、触发器或其他取值者。每行中操作严格从左到右分条执行，函数调用统一 `SELECT 函数(...) AS v`，序列参数统一 `'public.seq_contract'::regclass`。下表为可读性省略 SELECT/AS v，采证时展开为完整 SQL。

| 用例 | 完整操作顺序与预期值/副作用 | 恢复与继续性 |
| --- | --- | --- |
| first/cross | 新 A currval 失败；A nextval=10、currval=10；新 B currval 失败；B nextval=11；A currval 仍10、B currval=11；A nextval=12、B currval仍11。另并发无重设/CYCLE的 A/B各100次 nextval，收集200个值断言互异且范围内，不按网络返回顺序断言递增。 | 首次错误各自 I 下 `SELECT 1 AS ok`=1；并发取值前两连接 I，无其他取值者。A断开重连后currval失败；B最后值不受影响（并发轮次另记实际最后值）。 |
| rollback | A `BEGIN`；nextval=10；`INSERT INTO public.seq_rows VALUES (1,10)`；`ROLLBACK`；A currval=10；B `SELECT id,v FROM public.seq_rows ORDER BY id`=[]；B nextval=11。 | A I→T→I；A/B `SELECT 1 AS ok`成功，表为空而序列已消耗，不能仅验标签。 |
| set-true | 新 A setval(序列,40)=40、currval=40、nextval=41；setval(序列,50,true)=50、currval=50；B nextval=51；A currval仍50。 | 两种 true 形式分别取证，B currval=51；成功均 I。 |
| set-false | 新 A setval(序列,60,false)=60，A currval仍未初始化而报错；B nextval=60、currval=60；A nextval=61；A setval(序列,70,false)=70，A currval仍61；B nextval=70；A currval仍61、B currval=70。 | currval错误后A I且SELECT 1成功；验证false既不初始化也不覆盖已有连接值。 |
| set-rollback | A nextval=10；A `BEGIN`；setval(序列,80,true)=80；在A回滚前B nextval=81；A `ROLLBACK`；A currval=80；B nextval=82；B查seq_rows仍空。 | A回滚不撤销共享值或连接值，B在A事务未结束时的可见性单独记录，限定CACHE 1。 |
| bounds | A nextval=10；setval(序列,0)与setval(序列,1001,false)逐条失败，之后A currval仍10、B nextval=11；再setval(序列,1000,true)=1000，nextval因NO CYCLE耗尽报错，A currval仍1000；setval(序列,999,false)=999，nextval=999。 | 每个I下失败后SELECT 1成功；再独立从夹具初态A `BEGIN`、nextval=10、`INSERT INTO public.seq_rows VALUES (1,10)`、setval(序列,0)失败，A进入E，SELECT 1须25P02；扩展Sync仍E，ROLLBACK→I，表空、A currval=10、B nextval=11。 |
| missing/type/permission | 新A依次调用nextval('public.seq_missing'::regclass)、nextval('public.seq_rows'::regclass)，分别缺对象/非序列失败；B nextval=10证明未消耗有效序列。权限独立夹具预建非owner且无继承/公开授权的角色r；分别以无权限、仅USAGE、仅SELECT、仅UPDATE建立新连接，逐一调用nextval/currval/setval并记录目标权限规则。 | 缺对象与错误类型每次失败后SELECT 1成功；权限组由管理员先撤销全部序列权限、逐轮只授单项，UPDATE/USAGE组先nextval再currval；仅SELECT组首次currval应为未初始化错误，不能误报权限不足。缺生产身份/ACL能力时登记阻止开放，不用root成功冒充权限验证。 |

权限轮次由管理员在 I 下执行 `REVOKE ALL ON SEQUENCE public.seq_contract FROM PUBLIC,r`，分别不授予或执行 `GRANT USAGE ON SEQUENCE public.seq_contract TO r`、`GRANT SELECT ON SEQUENCE public.seq_contract TO r`、`GRANT UPDATE ON SEQUENCE public.seq_contract TO r`（四个独立初态，不累计授权）。r另有数据库CONNECT与public的USAGE，不能为owner/superuser。各轮均执行 `SELECT nextval('public.seq_contract'::regclass) AS v`、`SELECT currval('public.seq_contract'::regclass) AS v`、`SELECT setval('public.seq_contract'::regclass,40) AS v`：无权限全部拒绝；仅USAGE为10、10、权限拒绝；仅SELECT为权限拒绝、未初始化错误、权限拒绝；仅UPDATE为10、权限拒绝、40。每次失败后r执行SELECT 1成功，管理员随后nextval预期分别10、11、10、41，证明拒绝未偷偷改变水位；角色登录配置由后续测试夹具提供，不能以当前开发root认证冒充。

上表“序列”是 SQL 参数占位，必须替换为 `'public.seq_contract'::regclass`；first/cross、rollback 等均从独立夹具初态运行，bounds 的两个越界失败之间不取值。每例结束先对仍在 T/E 的连接 ROLLBACK，再在 I 下 DROP TABLE public.seq_rows、DROP SEQUENCE public.seq_contract；权限组额外撤销授权并删除测试角色。新例用新连接和新建对象，不能用 setval 冒充 currval 清空。对象缺失例的 seq_missing 必须初始不存在。

Q 与扩展 Parse/Bind/Describe/Execute 各取一组；扩展使用新 statement/portal、无参数、全量Execute后Sync。Parse/Bind/Describe不得消耗或设置序列值，首次Execute仅一次副作用；精确诊断阶段待采，扩展错误先Sync，再按I/E决定继续或ROLLBACK。记录值、别名v、bigint/OID20与固定描述、命令标签、SQLSTATE/错误原文/位置、I/T/E及每次观察连接值，typlen/typmod/格式/可空性等精确wire字段待采。成功SQLSTATE不适用；本轮不伪造参考结果。准备/描述不取值也须以另一连接后续nextval证明，不能为观察而在受测连接多调一次改变状态。

共存用例 `session.sequence.mysql.coexist` 关联 `SESSION-SEQUENCE/mysql`（待新增、未运行）：同一Server的MySQL M先执行 `CREATE TABLE seq_mysql (id BIGINT AUTO_INCREMENT PRIMARY KEY,v INT)`、`INSERT INTO seq_mysql(v) VALUES(1)`，保存返回自增值x并查 `SELECT id,v FROM seq_mysql ORDER BY id`；PG A/B跑独立first/cross后，M执行 `SELECT LAST_INSERT_ID()`仍x、`INSERT INTO seq_mysql(v) VALUES(2)`并查两行，按自身基线核对第二ID、类型/标志和状态。用M建真实原生序列 `CREATE SEQUENCE seq_native START WITH 5 NOCACHE`，分别 `SELECT NEXTVAL(seq_native)`=5、`SELECT LASTVAL(seq_native)`=5；PG另建seq_contract的setval不能改变它，再M NEXTVAL=6。PG不可用MySQL的LAST_INSERT_ID或AUTO_INCREMENT替代currval；不要求两方言自增ID连续或权限/错误相等。结束M删除seq_native/seq_mysql；若原生夹具也未通过，单列原生基线缺口。

#### 标识列生成契约

下列 `session.identity.pg.*` 均为**待新增、未运行**，关联 `SESSION-IDENTITY/pg`；每例独立重建夹具。A/B 为同库全新 PG 连接，UTF-8、UTC、search_path=public、autocommit、READ COMMITTED、初始 I，具有夹具所需权限；无其他写入者、触发器、取值者或重设。事务外执行以下两条 SQL，初始均空，除指定表外另表保持空；identity DDL 尚不可用时登记前置能力缺口，不能换 AUTO_INCREMENT 让正例通过。

```sql
CREATE TABLE public.identity_a (id bigint GENERATED ALWAYS AS IDENTITY (START WITH 10 INCREMENT BY 1 MINVALUE 1 MAXVALUE 1000 CACHE 1 NO CYCLE) PRIMARY KEY, note text UNIQUE);
CREATE TABLE public.identity_d (id bigint GENERATED BY DEFAULT AS IDENTITY (START WITH 10 INCREMENT BY 1 MINVALUE 1 MAXVALUE 1000 CACHE 1 NO CYCLE) PRIMARY KEY, note text UNIQUE);
```

下表每行从新夹具开始，SQL 按顺序分条发送。成功 INSERT 均追加 `RETURNING id,note`；表中给出该语句返回行，观察查询使用 `SELECT id,note FROM public.identity_a ORDER BY id` 或对应 identity_d，由指定连接执行；表名替换处同时替换 INSERT 和观察查询，不替换 SQL 方言。

| case_id 后缀 | 完整输入与目标行/副作用 | 失败恢复及后续生成 |
| --- | --- | --- |
| generate | A `INSERT INTO public.identity_a(note) VALUES ('omit')`→(10,omit)；`INSERT INTO public.identity_a(id,note) VALUES (DEFAULT,'default')`→(11,default)；B 观察为这两行。identity_d 从独立初态重复同组输入，目标相同。 | 均 I；两表序列独立。另独立初态 A `INSERT INTO public.identity_a DEFAULT VALUES RETURNING id,note`→(10,SQL NULL)，B观察同值。 |
| always-reject | A `INSERT INTO public.identity_a(id,note) VALUES (900,'bad') RETURNING id,note`须拒绝，B观察空；A `SELECT 1 AS ok`→1；随后 `INSERT INTO public.identity_a(note) VALUES ('after')`→(10,after)。 | 无 BEGIN 的拒绝后 I；另从新初态 BEGIN 后运行拒绝语句，E、后续SELECT 1门禁，扩展Sync仍E，ROLLBACK→I；表仍空，之后生成10。错误码、消息和阶段待采，不用现有 RETURNING 门禁冒充 identity 校验。 |
| explicit | A `INSERT INTO public.identity_a(id,note) OVERRIDING SYSTEM VALUE VALUES (900,'manual')`→(900,manual)；随后 `INSERT INTO public.identity_a(note) VALUES ('next')`→(10,next)；B观察按id排序为(10,next),(900,manual)。 | identity_d 从新初态分别测试无覆盖和SYSTEM覆盖，均人工900后生成10；分别取证，不累计初态。 |
| user-override | A `INSERT INTO public.identity_a(id,note) OVERRIDING USER VALUE VALUES (900,'ignored')`→(10,ignored)；随后 `INSERT INTO public.identity_a(note) VALUES ('next')`→(11,next)，B观察两行。identity_d 从新初态重复同组。 | 另新初态 A `INSERT INTO public.identity_a(id,note) OVERRIDING SYSTEM VALUE VALUES (DEFAULT,'default')`→(10,default)，证明覆盖不取消DEFAULT生成。 |
| null | A `INSERT INTO public.identity_d(id,note) VALUES (NULL,'bad') RETURNING id,note`须NOT NULL失败，B观察空；A SELECT 1成功；随后 `INSERT INTO public.identity_d(note) VALUES ('after')`→(10,after)。 | I下独立语句；显式NULL不转换成生成。ALWAYS显式NULL在无覆盖时的identity诊断另采，不借此推断NOT NULL先后。 |
| rollback | A BEGIN；`INSERT INTO public.identity_a(note) VALUES ('undone')`→(10,undone)；A观察有此行、B观察空；A ROLLBACK，A/B观察均空；B `INSERT INTO public.identity_a(note) VALUES ('kept')`→(11,kept)，A观察仅(11,kept)。 | A I→T→I；表回滚与序列不回退各有证据，RETURNING不是提交证明。 |
| failure-after-allocation | A `INSERT INTO public.identity_a(note) VALUES ('seed')`→(10,seed)；A BEGIN；`INSERT INTO public.identity_a(note) VALUES ('seed') RETURNING id,note`因note唯一键失败，无成功返回行；A进入E，SELECT 1须失败，扩展Sync仍E；A ROLLBACK；B观察仅(10,seed)；A `INSERT INTO public.identity_a(note) VALUES ('after')`→(12,after)，B最终观察(10,seed),(12,after)。 | 运行期分配11后约束失败不能退水位，也不能保留失败行。另从新夹具以autocommit重复seed及失败输入，错误后I、SELECT 1成功，后续仍生成12。 |

目录用例 `session.identity.pg.catalog` 使用同一初始夹具，执行以下无副作用查询，要求 id 的 attidentity 分别为 a/d、attnotnull=true、atttypid=20、atthasdef=false；note 的 attidentity为空且atthasdef=false。pg_get_serial_sequence 返回各自真实序列的可解析名称，不固定其拼写或 OID 数值；据返回的 OID 到 pg_class/pg_sequence 核对关系类型S、起点10、步长1、缓存1、无循环及范围1–1000。引用下列SQL的目录解析能力若未开放，登记阻止目录验收，不以伪行/硬编码常量补齐。

```sql
SELECT attname,attidentity,attnotnull,atttypid,atthasdef FROM pg_catalog.pg_attribute WHERE attrelid='public.identity_a'::regclass AND attnum>0 ORDER BY attnum;
SELECT attname,attidentity,attnotnull,atttypid,atthasdef FROM pg_catalog.pg_attribute WHERE attrelid='public.identity_d'::regclass AND attnum>0 ORDER BY attnum;
SELECT pg_get_serial_sequence('public.identity_a','id') AS seq_name;
SELECT pg_get_serial_sequence('public.identity_d','id') AS seq_name;
SELECT c.oid,c.relkind,s.seqtypid,s.seqstart,s.seqincrement,s.seqmin,s.seqmax,s.seqcache,s.seqcycle FROM pg_catalog.pg_class c JOIN pg_catalog.pg_sequence s ON s.seqrelid=c.oid WHERE c.oid=pg_get_serial_sequence('public.identity_a','id')::regclass;
SELECT d.objid,d.objsubid,d.refobjid,d.refobjsubid,d.deptype FROM pg_catalog.pg_depend d WHERE d.classid='pg_catalog.pg_class'::regclass AND d.refclassid='pg_catalog.pg_class'::regclass AND d.refobjid='public.identity_a'::regclass AND d.refobjsubid=1;
SELECT adnum,pg_get_expr(adbin,adrelid) AS expr FROM pg_catalog.pg_attrdef WHERE adrelid='public.identity_a'::regclass ORDER BY adnum;
```

序列参数查询对identity_d替换表名重复，seqtypid=20。依赖查询须找到真实所关联序列→identity_a.id 的 i 边，objsubid=0，refobjsubid=1；对identity_d替换表名重复，所关联序列不同。pg_attrdef 查询同样对两表执行，目标均零行，与atthasdef=false对照；此为上述官方源码推导，精确参考目录输出仍待实测，不能把该行集当“必有nextval”断言。完成目录查询后再跑各表第一条生成插入，均应10，证明内省未取值；当前实现无列拥有边的测试仅是现状，不能把零行当目标通过。

Q 与扩展路径各从新初态取证：扩展新建命名statement/portal、无参数、文本结果、全量Execute后Sync。独立 `session.identity.pg.describe-no-allocation` A先只Parse/Bind/Describe `INSERT INTO public.identity_a(note) VALUES ('prepared') RETURNING id,note`，B随后执行 `INSERT INTO public.identity_a(note) VALUES ('observer') RETURNING id,note` 应返回(10,observer)，A再Execute应返回(11,prepared)，B观察仅这两行；保存描述及结果，避免观察调用nextval改变受测状态。列名id/note，成功返回类型bigint/OID20、text/OID25，值不得SQL NULL的id与允许NULL的note分别核对；精确表OID/属性号、typlen/typmod、格式、可空性、标签和完整S/V/C/M/D/H/P诊断及阶段待采。成功SQLSTATE不适用，拒绝语句无成功返回行；产品能力拒绝与PG参考identity/NOT NULL/唯一键错误分别登记。仍在T/E的连接先ROLLBACK，再在I下DROP TABLE public.identity_a及public.identity_d，按真实关联检查内建序列随表清理，不另手工DROP来掩盖孤儿。清理/生命周期未满足时记录工作流6缺口；下例不复用残留对象。

共存用例 `session.identity.mysql.coexist` 关联 `SESSION-IDENTITY/mysql`，分类MySQL专属（待新增、未运行）：同一Server的M连接固定数据库、autocommit与sql_mode（记录完整值，排除NO_AUTO_VALUE_ON_ZERO），先 `CREATE TABLE identity_mysql (id BIGINT AUTO_INCREMENT PRIMARY KEY,note VARCHAR(30))`；`INSERT INTO identity_mysql(note) VALUES ('before')`，保存生成值x、`SELECT LAST_INSERT_ID()`及 `SELECT id,note FROM identity_mysql ORDER BY id`。PG A/B执行独立generate和always-reject组后，M的LAST_INSERT_ID仍x；M继续 `INSERT INTO identity_mysql(id,note) VALUES (900,'manual')`、`INSERT INTO identity_mysql(note) VALUES ('after')`、`INSERT INTO identity_mysql(id,note) VALUES (NULL,'null')`，逐次保存自增响应/最后值、真实行、类型标志、状态及对应MySQL/既有原生基线。第二次自动值与NULL分配的精确数值按自身基线取证，不能要求与PG的10/11相等或套用PG显式值不rebase策略。PG目录中该原生表的attidentity须为空，无虚构列拥有序列；M可继续SELECT 1，结束DROP TABLE identity_mysql。任何原生基线缺口单列，不以MySQL近似语法证明PG支持。

#### 事务提交与回滚契约

以下 `session.txn.pg.*` 均为**待新增、未运行**，关联 `SESSION-TXN/pg`；每例在独立夹具中开始。A/B 是同库全新 PG 连接，UTF-8、UTC、search_path=public、autocommit、初始 I，具有夹具所需权限，无其他写入者/触发器/序列。参考端 READ COMMITTED；产品端记录真实隔离设置及差异，不用不受支持的 SET 成功假设替代证据。A 在事务外逐条执行下列 setup，B 确认初态 S0 后才开始受测序列；DDL setup 不作为事务型 DDL 验收。

```sql
CREATE TABLE public.tx_contract (id integer PRIMARY KEY, value integer NOT NULL);
INSERT INTO public.tx_contract VALUES (1,10),(2,20);
```

观察查询 O 为 `SELECT id,value FROM public.tx_contract ORDER BY id`，每次由指定连接独立发送，返回列 id/value、int4/OID23、非 NULL，文本格式；typlen/typmod、表/属性身份及精确 wire 元数据待采且沿第 3 章验收。S0=[(1,10),(2,20)]，S1=[(1,10),(2,20),(3,30)]，U=[(1,11),(2,20)]，D=[(1,11)]。以下表中的 A/B 行状态均指实际执行 O 的结果；B 每条 O 都是新的 autocommit 查询，末尾 I。A 的 O 不改变其所在事务的 T/I。

| case_id 后缀 | 顺序输入、每步副作用观察及 ReadyForQuery 目标 |
| --- | --- |
| commit | A `BEGIN`→T，A/B O 均 S0；A `INSERT INTO public.tx_contract VALUES (3,30)`→T，A O=S1、B O=S0；A `COMMIT`→I，A/B O 均 S1；再开全新 C 执行 O=S1、I。提交前后保存完整行，不能仅数行或看 COMMIT 标签。 |
| rollback | A `BEGIN`→T，A/B O=S0；A `UPDATE public.tx_contract SET value=11 WHERE id=1`→T，A O=U、B O=S0；A `DELETE FROM public.tx_contract WHERE id=2`→T，A O=D、B O=S0；A `ROLLBACK`→I，A/B O=S0；A `SELECT 1 AS ok`→1、I。必须同时恢复更新前值和被删除行。 |
| implicit | 无 BEGIN，A `INSERT INTO public.tx_contract VALUES (3,30)`→I，A/B O=S1；A `INSERT INTO public.tx_contract VALUES (4,40),(1,99)` 触发唯一键失败，A→I、B O 仍 S1（id=4 不得残留），A `SELECT 1 AS ok`→1、I。23505 为唯一键目标，精确消息/阶段及本地原子性待采。 |
| repeated-begin | A `BEGIN`→T；`INSERT INTO public.tx_contract VALUES (3,30)`→T，A O=S1、B O=S0；A再次 `BEGIN` 应警告、仍 T，A O=S1、B O=S0；A `ROLLBACK`→I，A/B O=S0。产品若拒绝则另存产品限制、恢复后 S0，不得当作参考目标通过。 |
| outside-end | 初始 I，A依次 `COMMIT`、`ROLLBACK`，各命令应警告、I，每步 A/B O=S0；A再 `BEGIN`→T、`ROLLBACK`→I、`SELECT 1 AS ok`→1。警告内容与精确 SQLSTATE 待采，无成功结果集。 |
| disconnect | A `BEGIN`→T；`UPDATE public.tx_contract SET value=11 WHERE id=1`→T；`DELETE FROM public.tx_contract WHERE id=2`→T；`INSERT INTO public.tx_contract VALUES (3,30)`→T；A O=[(1,11),(3,30)]、B O=S0；不发送 COMMIT，分别以 Terminate 与直接关闭 socket 的独立子例结束 A。确认服务端识别关闭及清理完成后，B O=S0，新 A2 O=S0、I，A2 `SELECT 1 AS ok`→1。已关闭 A 无 ReadyForQuery，超时记失败而非成功；网络失联检测时限待环境取证。 |
| error-recover | A `BEGIN`→T；`UPDATE public.tx_contract SET value=11 WHERE id=1`→T；`INSERT INTO public.tx_contract VALUES (1,99)` 唯一键失败→E；B O=S0；当前产品 A `SELECT 1 AS ok` 和 `COMMIT` 均 25P02、E，不得产生新写入；扩展路径 Sync 后仍 E；A `ROLLBACK`→I，A/B O=S0，A `SELECT 1 AS ok`→1。PG参考端失败后COMMIT独立取证，不把产品策略作为PG预期；本行保留批次13/14的后续入口。 |

每条成功控制命令和无 RETURNING 的写入无结果集；另存 BEGIN/COMMIT/ROLLBACK 及 INSERT/UPDATE/DELETE 的 CommandComplete、影响行数与通知，不用标签代替 O。成功 SQLSTATE 不适用；失败无成功返回行，按首块记录 S/V/C/M、适用 D/H/P、原 SQL 位置和发生阶段。错误/警告精确文本及阶段未采，不能从共享错误字符串补造参考结果。

Q 路径逐条 SQL 一个消息；扩展复验每条 SQL 单独命名 Parse、Bind（零参数）、Execute（全量）、Sync，使用文本结果且在 Sync 收集表中目标 I/T/E；不得在 Execute 后期待立即有 ReadyForQuery。失败时先 Sync 同步再发 ROLLBACK；已准备 portal 在状态变化后 Execute 的门禁另交失败态任务。两路径各从新夹具开始，记录完整消息及 O 的值，不复用已提交的行。清理先将仍存活的 T/E 连接 ROLLBACK 至 I，确认断连清理完成，再事务外 `DROP TABLE public.tx_contract` 并关闭连接；出现未结束事务/残留写入须记失败。

共存 `session.txn.mysql.coexist`（待新增、未运行）关联 `SESSION-TXN/mysql`，限定普通 DML 共享语义。M/N 为同一 Server 的独立 MySQL 连接，固定数据库，`SET autocommit=1`，记录真实 transaction_isolation 和完整 sql_mode；M事务外 `CREATE TABLE tx_contract_mysql (id INT PRIMARY KEY,value INT NOT NULL)`、`INSERT INTO tx_contract_mysql VALUES (1,10),(2,20)`，N确认初态S0。MySQL观察 OM=`SELECT id,value FROM tx_contract_mysql ORDER BY id`。M `START TRANSACTION`、`UPDATE tx_contract_mysql SET value=11 WHERE id=1`、`DELETE FROM tx_contract_mysql WHERE id=2`，M OM=D、N OM=S0；保持 M 事务期间 PG A/B 在另一张 tx_contract 上执行独立 commit 与 error-recover 组，M仍为D、N仍S0，M `ROLLBACK` 后两者S0。随后 M `START TRANSACTION`、`INSERT INTO tx_contract_mysql VALUES (3,30)`，M OM=S1、N OM=S0；M `COMMIT` 后 N OM=S1；M/N `SELECT 1 AS ok` 均1。保存 MySQL 自身 OK/ERR、状态位、列类型标志及参考 MySQL/既有原生基线，不套用 PG I/T/E、25P02 或重复 BEGIN 策略；失败组只在 PG 执行。最后 M/N先ROLLBACK再事务外DROP TABLE tx_contract_mysql。前置建表/事务能力不满足时登记缺口，不改写方言语法让测试通过。

#### 失败事务恢复契约

下列 `session.txn.failed.pg.*` 是**待新增、未运行**用例，关联 `SESSION-TXN-FAILED/pg`，继承第1章证据格式。每组独立从新 PG 连接 A/B 开始，同库、UTF-8、UTC、search_path=public、初始 I、客户端自动提交开启但显式发送 BEGIN；参考 PG 固定 READ COMMITTED，本地记录实际隔离配置并证明 B 每次为新事务快照。事务外准备 `CREATE TABLE public.tx_failed_contract (id INT PRIMARY KEY,value INT NOT NULL)`、`INSERT INTO public.tx_failed_contract VALUES (1,10),(2,20)`。观察 O 为 `SELECT id,value FROM public.tx_failed_contract ORDER BY id`，初态 S0=[(1,10),(2,20)]；本表无序列/identity，避免把其非回滚值混入断言。

公共失败前缀 P：A `BEGIN`→T；`UPDATE public.tx_failed_contract SET value=11 WHERE id=1`→T；`INSERT INTO public.tx_failed_contract VALUES (3,30)`→T；A O=[(1,11),(2,20),(3,30)]，B O=S0；随后 F 分两轮独立夹具：唯一键轮 `INSERT INTO public.tx_failed_contract VALUES (1,99)`，目标23505；表达式轮 `SELECT 1/0 AS bad`，目标22012。各轮无成功行，A→E，B O仍S0。表达式不产生预期异常时登记 PG 语义缺口，不把 NULL/警告当作通过；记录发生于解析、绑定、执行还是输出阶段。

| 用例与完整后续输入 | 目标结果、副作用和恢复 |
| --- | --- |
| rollback-success：P、F 后 A 逐条 `SELECT 1 AS ok`、`SELECT id,value FROM public.tx_failed_contract ORDER BY id`、`INSERT INTO public.tx_failed_contract VALUES (4,40)`、`UPDATE public.tx_failed_contract SET value=99 WHERE id=2`、`DELETE FROM public.tx_failed_contract WHERE id=2`。再 `ROLLBACK`、O、`SELECT 1 AS ok`；再 `BEGIN`、`INSERT INTO public.tx_failed_contract VALUES (5,50)`、`COMMIT`、O。 | E 中每条普通读写均25P02、仍E，无成功行；B每步O=S0。完整ROLLBACK→I，A/B O=S0且SELECT 1=1；新事务提交后两者 O=[(1,10),(2,20),(5,50)]，证明新写入已提交可见而旧写入未泄漏。控制命令和无RETURNING写入无结果集。 |
| commit-exit：独立 P、F；A `COMMIT`、O、`SELECT 1 AS ok`。 | PG参考目标为完成标签ROLLBACK、I、O=S0、SELECT 1=1。当前产品预期限制为25P02/E，不能在E执行O冒充成功；须先另发ROLLBACK→I，再A/B O=S0、SELECT 1=1。参考目标与产品限制两份结果并存，不把限制验收记为PG兼容通过。 |
| prepared-before-error：A BEGIN后，在T中以扩展Parse/Bind准备未执行的命名读 `SELECT id,value FROM public.tx_failed_contract ORDER BY id` 和写 `INSERT INTO public.tx_failed_contract VALUES (4,40)`，各以不同statement/portal名、零参数、文本结果，Sync后T；再执行P中两条写入和F。 | 错误后先Sync，仍E；依次对原读/写portal Execute（全量），每次收错误后Sync。目标每次25P02、E、无DataRow/写入，B O=S0。ROLLBACK后A/B O=S0，SELECT 1=1。现有源码缺Execute重检仅是风险；Bind/portal已被关闭或失效时保存实际阶段/诊断，不能宣称已覆盖预期25P02。此组不复用portal来证明恢复后成功，新建语句另验。 |
| connection-isolation：保持A在P、F后的E，B `SELECT 1 AS ok`、O；新建C执行 `SELECT 1 AS ok`、O；B `BEGIN`、`UPDATE public.tx_failed_contract SET value=21 WHERE id=2`、`ROLLBACK`、O；最后A ROLLBACK、O、SELECT 1。 | B/C不接收A的25P02，查询值分别1与S0；B独立T→I且回滚后O=S0，不访问A已锁的id=1。A仍E直到自身ROLLBACK，之后所有存活连接O=S0、可查询。超时或跨连接失败标志传播均记失败。 |
| outside-error：独立初态I，A分别发送唯一键F和表达式F，每次随后 `SELECT 1 AS ok`、O。 | 每个失败自动结束该单语句事务，下一查询成功且I，O=S0；不得错误要求完整ROLLBACK。失败码分别23505/22012，实际诊断另采；不将本组扩展为无BEGIN的多消息隐式批次。 |

rollback-success、commit-exit、outside-error 的 Q 模式每条SQL一个消息；扩展模式另从新夹具开始，每条SQL使用独立命名 Parse、零参数 Bind、全量 Execute、Sync，失败后先Sync再继续控制命令，在Sync读取I/T/E。prepared-before-error为专门跨状态复验，不以“在E中新Parse被拒绝”替代。记录每次错误S/V/C/M及适用D/H/P、原SQL和阶段，精确文本/位置、描述类型OID/typmod/可空性、命令标签、影响行数均待实采；行值、状态和副作用是通过必需项，不能只统计行数。控制命令无结果列，成功SQLSTATE不适用。清理对所有存活T/E连接先ROLLBACK至I，再事务外DROP TABLE public.tx_failed_contract并关闭连接；清理失败、缺所需隔离或协议能力须记缺口。

保存点仅保留独立能力门槛用例 `session.txn.failed.pg.savepoint`（待新增、未运行）：同夹具A `BEGIN`、`UPDATE public.tx_failed_contract SET value=11 WHERE id=1`、`SAVEPOINT s`、`INSERT INTO public.tx_failed_contract VALUES (3,30)`、唯一键F、`ROLLBACK TO SAVEPOINT s`、O、`COMMIT`。PG目标E→T，A O=[(1,11),(2,20)]，B提交前S0、提交后同A；保存点之后的id=3撤销而之前的id=1保留。产品能力未开放时记录拒绝阶段/SQLSTATE（精确码待取证），停止后续成功假设并完整ROLLBACK，A/B O=S0、SELECT 1=1；不得把ROLLBACK TO悄悄当完整ROLLBACK。此用例不表示本轮要求实现保存点。

共存 `session.txn.failed.mysql.coexist`（待新增、未运行）关联 `SESSION-TXN-FAILED/mysql`，只证明PG失败策略不外溢。M/N为同一Server独立MySQL连接，记录真实sql_mode与transaction_isolation，`SET autocommit=1`；事务外 `CREATE TABLE tx_failed_mysql (id INT PRIMARY KEY,value INT NOT NULL)`、`INSERT INTO tx_failed_mysql VALUES (1,10),(2,20)`。OM=`SELECT id,value FROM tx_failed_mysql ORDER BY id`。M `START TRANSACTION`、`UPDATE tx_failed_mysql SET value=21 WHERE id=2`，M OM=[(1,10),(2,21)]、N OM=S0；此时PG A执行P、F、门禁拒绝、ROLLBACK整组，M再次OM仍自己的值、N仍S0，M SELECT 1=1；M ROLLBACK后M/N OM=S0。再M START TRANSACTION、INSERT INTO tx_failed_mysql VALUES (5,50)、COMMIT，N OM=[(1,10),(2,20),(5,50)]。保存MySQL自身OK/ERR、状态位、类型标志和既有原生/真实MySQL基线，不向M套PG25P02或假定MySQL遇同类错误会整事务失败。清理M/N先ROLLBACK，再事务外DROP TABLE tx_failed_mysql。

#### 协议同步与事务恢复边界

下列 `session.txn.sync.*` 均为**待新增、未运行**用例，复用第1章取证格式和前块能力ID，不表示新增测试文件。每组新建PG连接A/B，同库UTF-8、UTC、search_path=public、初始I；B自动提交且每次新快照。参考PG设READ COMMITTED，本地记录真实隔离配置。事务外准备 `CREATE TABLE public.tx_sync_contract (id INT PRIMARY KEY,value INT NOT NULL)`、`INSERT INTO public.tx_sync_contract VALUES (1,10),(2,20)`。O=`SELECT id,value FROM public.tx_sync_contract ORDER BY id`，S0=[(1,10),(2,20)]。

消息记法PBX(sql)：新唯一命名statement与portal，Parse零OID/零参数、Bind文本结果、Execute max_rows=0，**不自动附Sync**；参数例外另注明。成功响应保留ParseComplete、BindComplete及CommandComplete/行值；只有Sync才期待扩展ReadyForQuery。等待Execute结果可用Flush取输出，Flush不作为事务或恢复边界。每组必须保存发送/接收次序与同步计数，不以超时推断消息被丢弃。

| 用例与完整输入/初始状态 | 目标值、副作用与恢复 |
| --- | --- |
| explicit-error：A Q BEGIN→T；PBX(`UPDATE public.tx_sync_contract SET value=11 WHERE id=1`)，Sync→T；PBX(`INSERT INTO public.tx_sync_contract VALUES (1,99)`)触发重复键；随后PBX(`INSERT INTO public.tx_sync_contract VALUES (3,30)`)、Q `DELETE FROM public.tx_sync_contract`、Sync。 | 首错23505；后两条完全丢弃，无其成功响应。ReadyForQuery(E)，B O=S0。再A Q O→25P02/E；PBX(`INSERT INTO public.tx_sync_contract VALUES (4,40)`)、Sync→25P02/E，B O=S0。A Q ROLLBACK→I，A/B O=S0、SELECT 1 AS ok=1；再新PBX(`INSERT INTO public.tx_sync_contract VALUES (5,50)`)、Sync→I，两者O=[(1,10),(2,20),(5,50)]。 |
| implicit-error：无BEGIN，A PBX(`UPDATE public.tx_sync_contract SET value=11 WHERE id=1`)，Flush收完成；同段PBX(`INSERT INTO public.tx_sync_contract VALUES (1,99)`)，后续PBX(`INSERT INTO public.tx_sync_contract VALUES (3,30)`)、Sync。 | 首错23505，后续写被跳过；Sync后I。A/B O=S0，证明成功前缀也撤销；A Q SELECT 1=1且I，不要求额外ROLLBACK。若UPDATE已永久提交即未满足目标，不能以I或错误恢复通过掩盖。 |
| success-boundary：新夹具无BEGIN，A PBX(`UPDATE public.tx_sync_contract SET value=11 WHERE id=1`)、Flush读完成，B O；再PBX(`INSERT INTO public.tx_sync_contract VALUES (3,30)`)、Sync，A/B O。另新夹具先Q BEGIN，再执行相同两PBX/Sync及B O，最后Q ROLLBACK。 | 隐式组Sync前B S0，之后I且A/B O=[(1,11),(2,20),(3,30)]。显式组Sync后T、B仍S0，ROLLBACK后I且A/B S0。对比证明Sync对两类事务的不同作用。 |
| parse/bind-errors：分别从显式BEGIN或无BEGIN起，先PBX(`UPDATE public.tx_sync_contract SET value=11 WHERE id=1`)、Flush读完成，无中间Sync。Parse轮发送 `SELECT FROM`；Bind轮先Parse `SELECT id FROM public.tx_sync_contract WHERE id=$1`、OID=[23]，再Bind文本参数 `bad`，不Execute。每轮再PBX(`INSERT INTO public.tx_sync_contract VALUES (3,30)`)、Sync。 | 参考目标Parse为42601，Bind为22P02；实际失败阶段及字段待采，不能把产品0A000当参考语法通过。后续写丢弃，显式组E、B S0，ROLLBACK后两者S0；隐式组I、两者S0且SELECT 1=1。失败不发生或发生在不同阶段须记录缺口，不跳过该轮。 |
| prebound/mixed-entry：显式T中先PBX写id=1为11；Parse/Bind未执行读O与写 `INSERT INTO public.tx_sync_contract VALUES (4,40)` 两命名portal，Sync→T。分两轮首错：Q重复键，或新PBX重复键后Sync；再对原portal分别Execute、各Sync。 | 首错后两入口均E。原portal若仍可寻址，不得输出缓存成功行/写入，目标失败态拒绝25P02/E；若已因错误销毁，另存真实生命周期错误且不得宣称已覆盖门禁。B每步S0，最后Q ROLLBACK→I、A/B S0，再新建PBX SELECT 1、Sync成功且I。 |
| cancel-boundary：显式/隐式各独立夹具先PBX更新id=1为11、Flush收完成；下一PBX为 `SELECT id,value FROM public.tx_sync_contract ORDER BY id`。后续实施测试须以受控执行屏障暂停该Execute，确认已活动再用本连接BackendKeyData发送CancelRequest；原连接确认取消错误后，发送PBX写id=3、Sync。 | 没有活动屏障或查询先完成时本轮不具备取消证据，不以idle cancel顶替。有效取消诊断目标57014，后续写跳过；显式E需ROLLBACK后S0，隐式I直接O=S0，两者SELECT 1成功。另在空闲I发送取消，随后新PBX SELECT 1、Sync应成功且I。屏障为待新增测试能力，不使用假定pg_sleep支持或新生产接口。 |

每组另保存首错S/V/C/M与适用D/H/P、阶段、Q/扩展状态、命令标签、影响行数、完整DataRow及描述；O的id/value声明整数，精确OID/typmod/可空性与错误文本/位置待两端采集，控制命令无结果列。成功SQLSTATE不适用。清理所有存活T/E连接先Sync恢复消息处理，再ROLLBACK至I，事务外DROP TABLE public.tx_sync_contract；若连接已失效由工作流7的关闭屏障验证撤销，不假装Sync成功。

共存 `session.txn.sync.mysql.coexist`（待新增、未运行）：同Server独立MySQL M/N，记录sql_mode、transaction_isolation，SET autocommit=1；事务外 `CREATE TABLE tx_sync_mysql (id INT PRIMARY KEY,value INT NOT NULL)`、`INSERT INTO tx_sync_mysql VALUES (1,10),(2,20)`。OM=`SELECT id,value FROM tx_sync_mysql ORDER BY id`。M START TRANSACTION、UPDATE tx_sync_mysql SET value=21 WHERE id=2，M OM=[(1,10),(2,21)]、N OM=S0；保持M事务，PG独立表执行explicit-error和implicit-error，M/N再次OM保持各自值、SELECT 1=1。M ROLLBACK后两者S0；再M START TRANSACTION、INSERT INTO tx_sync_mysql VALUES (5,50)、COMMIT，N OM=[(1,10),(2,20),(5,50)]。记录MySQL自身OK/ERR、状态位/类型标志及真实MySQL或既有原生基线，不向MySQL发送Sync或套用PG失败策略；M/N先ROLLBACK再事务外DROP TABLE tx_sync_mysql。

#### 事务型建表能力门槛

下列 `session.txddl.pg.*` 为**待新增、未运行**的用例规格，关联 `SESSION-TX-DDL/pg`；当前产品门禁基线与未来原型成功目标分开采证。A/B 为同一全新隔离测试库的 PG 两连接，UTF-8、UTC、search_path=public、初始 I、具夹具权限、无其他写入者；参考端 READ COMMITTED，产品端记录实际隔离/存储与差异。每例独立在事务外逐条执行 `CREATE TABLE public.txddl_sentinel (id integer PRIMARY KEY,v integer NOT NULL)`、`INSERT INTO public.txddl_sentinel VALUES (1,10)`；保证 public.txddl_probe 不存在。S=`SELECT id,v FROM public.txddl_sentinel ORDER BY id`，初始双方 [(1,10)]。D=`SELECT n.nspname,c.relname,c.relkind FROM pg_catalog.pg_class AS c JOIN pg_catalog.pg_namespace AS n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname='txddl_probe'`，初始双方零行。P=`SELECT id,v FROM public.txddl_probe ORDER BY id`；未建/回滚后 P 应缺关系而非成功零行，精确诊断待参考采集。

每条 SQL 单独发送 Q 并收 ReadyForQuery；另独立用同 SQL 的命名 Parse/Bind（无参数、text 结果）、Execute(max_rows=0)、Sync 重跑；任何 Parse/Bind 早拒绝也要记录真实阶段，不能只因未 Execute 而省略副作用检查。扩展错误先 Sync 恢复消息处理；显式 E 仍须 ROLLBACK。目录 D 失败或提供器不支持不能改成固定行，应登记缺证，不把内省缺口当表不存在。

| 用例与完整操作顺序（每例独立初态） | 目标、副作用与恢复 |
| --- | --- |
| rollback：A `BEGIN`、`UPDATE public.txddl_sentinel SET v=11 WHERE id=1`、`CREATE TABLE public.txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)`、`INSERT INTO public.txddl_probe VALUES (1,20)`；每步 A/B 采 S/D，插入后 A 采 P；A `ROLLBACK` 后双方采 S/D/P | 参考/未来原型：建表后 A D 一行 public/txddl_probe/r，插入后 A P=[(1,20)]、A S=[(1,11)]；B S 初值、D 零行。回滚后双方 S 初值、D 零行、P 缺关系；A I，SELECT 1=1。同名表在事务外可重新建/删，证明无名称残留。当前产品应在 CREATE 返回0A000/E，跳过其后的INSERT，按 reject 恢复；不能将产品拒绝当原型成功。 |
| commit：同 rollback 前缀至插入后，以 A `COMMIT` 替换 ROLLBACK；双方再 S/D/P | 提交前观察同上；提交后 A/B S=[(1,11)]、D一行、P=[(1,20)]，A I。目录列/主键再用 `SELECT column_name,data_type,is_nullable FROM information_schema.columns WHERE table_schema='public' AND table_name='txddl_probe' ORDER BY ordinal_position`、`SELECT constraint_type FROM information_schema.table_constraints WHERE table_schema='public' AND table_name='txddl_probe' ORDER BY constraint_type` 采真实结果，目标 id/v 为 integer、均 NOT NULL，单一PRIMARY KEY；精确类型/元数据按第3/6章采证。当前门禁下此成功分支不可执行，记未验证。 |
| reject：A BEGIN、UPDATE哨兵为11，分别独立发送 `CREATE TABLE public.txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)`、`DROP TABLE public.txddl_sentinel`、`ALTER TABLE public.txddl_sentinel ADD COLUMN extra integer`、`TRUNCATE TABLE public.txddl_sentinel` 四轮；各轮首错后 A `SELECT 1`、`COMMIT`，B S/D，再 A ROLLBACK，双方 S/D | 产品当前限制目标首错0A000/E，之后25P02/E，不隐式提交、不得建表/删表/加列/清空。ROLLBACK→I、S初值、D零行，双方 `SELECT id,v FROM public.txddl_sentinel ORDER BY id` 成功且 `SELECT extra FROM public.txddl_sentinel` 缺列（精确诊断待采）；A SELECT 1成功。PG 对这些合法表级命令应按各自行为执行并可回滚，不能以产品0A000冒充参考预期；该四轮是限制基线，不开放完整四类能力。 |
| write-failure：仅参考/未来原型，A `BEGIN`、`UPDATE public.txddl_sentinel SET v=11 WHERE id=1`、`CREATE TABLE public.txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)`、`INSERT INTO public.txddl_probe VALUES (1,20)`，再 `INSERT INTO public.txddl_probe VALUES (1,30)` | 目标重复键失败，SQLSTATE目标23505、E；Sync不退出显式E，后续SELECT 1目标25P02。B始终S初值、D零行；A ROLLBACK后双方S初值、D零行、P缺关系，再SELECT 1成功并可同名建表。记录DDL已完成而用户事务失败的清理，不以DDL job自身回滚替代；当前产品在CREATE早拒绝则单列未覆盖此失败点。 |
| database-restrict：A连接夹具库，另有事务外预建的空库 txddl_drop_db 且无连接；txddl_create_db不存在。每轮 A BEGIN、UPDATE哨兵为11，分别 `CREATE DATABASE txddl_create_db` 或 `DROP DATABASE txddl_drop_db`；首错后Sync（扩展组）、ROLLBACK | PG自身禁止事务块，产品六类门禁目标0A000/E；PG精确码/消息/阶段待参考采证，不套产品码。双方 `SELECT datname FROM pg_catalog.pg_database WHERE datname IN ('txddl_create_db','txddl_drop_db') ORDER BY datname` 始终仅txddl_drop_db，回滚后S初值、A SELECT 1成功；夹具库不能作为DROP目标。 |

rollback/commit/write-failure 每次目录观察必须在对应阶段完成后再继续下一步；B 若做直接 P 名称解析或访问而阻塞，须用独立观察轮次、受控超时/取消并记录等待与锁时序，不要求所有访问立即报缺关系，不让观察查询拖住 A 的 COMMIT/ROLLBACK。未提交时绝不能成功读到对象/数据；目录查询是否阻塞与精确结果也要两端采集，无法建立可靠观察则结论未知。A 失败态不强行读目录，改由 B 观察及 A 恢复后核实。错误后清理先 Sync（若需要）再 ROLLBACK→I；事务外按存在性查询结果逐条 DROP TABLE public.txddl_probe、DROP TABLE public.txddl_sentinel；从非目标库的管理连接 DROP DATABASE txddl_drop_db，若失败残留 txddl_create_db 则先存证再删除。删除是测试清理，不能算原子性证据。

故障清理另列原型必备缺证项：在提交前、已提交确认前、会话关闭/取消与DDL元数据发布边界设可控故障点（测试能力**待新增**，不假定接口已存在），以rollback同一输入和S/D/P验证无泄露/残留；提交确认不确定时记录真实事务结局，不声称自动回滚。成功CREATE/INSERT/control无结果列，标签/影响行数单存；S/P整数列、D及目录元数据、所有错误S/V/C/M及适用D/H/P、I/T/E/阶段/取消时序按公共模板采证，精确OID/typmod/对象身份与PG18.6诊断未采，不编数值。崩溃恢复须另有持久存储证据，本轮原型规格不代替。

共存 `session.txddl.mysql.coexist`（待新增、未运行），关联 `SESSION-TX-DDL/mysql`：同Server独立MySQL库，M/N记录sql_mode、隔离与数据库，SET autocommit=1；事务外 `CREATE TABLE txddl_mysql (id INT PRIMARY KEY,v INT NOT NULL)`、`INSERT INTO txddl_mysql VALUES (1,10)`。M START TRANSACTION、`UPDATE txddl_mysql SET v=12 WHERE id=1`，M `SELECT id,v FROM txddl_mysql ORDER BY id`=[(1,12)]、N同查询=[(1,10)]；交错PG reject/恢复后重复双方读取，结果不变。M ROLLBACK后双方[(1,10)]；M `START TRANSACTION`、`INSERT INTO txddl_mysql VALUES (2,20)`、`COMMIT` 后N读[(1,10),(2,20)]。原MySQL Query及 Prepare `SELECT v FROM txddl_mysql WHERE id=?`/Execute整数1分别核对值、类型/flags及状态位，不能套PG门禁。另用独立轮次，重建同结构txddl_mysql且仅有(1,10)，M `START TRANSACTION`、`UPDATE txddl_mysql SET v=13 WHERE id=1`、`CREATE TABLE txddl_mysql_ddl (id INT PRIMARY KEY)`、ROLLBACK，采双方数据/元数据并与真实MySQL或既有原生基线比较其隐式提交行为；这里不要求PG式回滚，也不预报实测结论。M/N回滚到空闲后事务外DROP两个MySQL表，记录恢复基线。

#### 章节验收与后续交接

文档验收逐行追溯上表十四族与来源四步，确认各块五项责任齐全，成功/拒绝、完整输入/初态、结果/待采字段、写后查询、恢复/清理及MySQL自身基线可定位。下面三个贯通用例为**待新增、未运行**的交接抽查定义，不能代替各族专项。PG A/B同一全新专用库、UTF-8、UTC、search_path=public、I，B每次新自动提交查询，无其他写者；事务外逐条 `CREATE TABLE public.handoff_pg (id integer PRIMARY KEY,v integer)`、`INSERT INTO public.handoff_pg VALUES (1,10)`。副作用查询 S 为 `SELECT id,v FROM public.handoff_pg ORDER BY id`，初值[(1,10)]；所有SQL独立发送。

| 用例 | 完整顺序与目标 | 副作用、恢复及未采字段 |
| --- | --- | --- |
| session.handoff.pg.commit | A BEGIN、`UPDATE public.handoff_pg SET v=11 WHERE id=1`、A/B各S、COMMIT、A/B各S、`SELECT 1 AS ok` | 提交前A[(1,11)]、B初值，提交后双方[(1,11)]，最后1；I→T→I。独立扩展变体在BEGIN前Parse命名h：`UPDATE public.handoff_pg SET v=$1 WHERE id=$2`，OID[23,23]；BEGIN后Bind新portal/text[11,1]、Describe、Execute(0)、Sync应T，再按原顺序观察/COMMIT。无RETURNING，UPDATE无结果列，影响行数目标1；S整数列int4，精确wire字段/标签实采。 |
| session.handoff.pg.reject-recover | 重建初值；A BEGIN、`UPDATE public.handoff_pg SET v=12 WHERE id=1`、`CREATE TABLE public.handoff_blocked (id integer)`；扩展拒绝变体在T中新Parse该DDL/无参数，错误后Sync；A `SELECT 1 AS ok`、ROLLBACK、双方S、A `SELECT 1 AS ok` | 产品限制目标0A000/E，Sync仍E；后续查询25P02，ROLLBACK后I/双方初值/1。B在各阶段S均初值，恢复后双方 `SELECT relname FROM pg_catalog.pg_class WHERE relname='handoff_blocked'` 零行，事务外直接 `SELECT id FROM public.handoff_blocked` 应缺对象且下一SELECT1可继续。精确缺对象错误/原始字段待采。PG参考普通建表应成功并可回滚，另存预期；不要求参考报产品限制。 |
| session.handoff.mysql.coexist | 同Server M/N独立MySQL库，SET autocommit=1，记录sql_mode/隔离/数据库；事务外 `CREATE TABLE handoff_mysql (id INT PRIMARY KEY,v INT)`、`INSERT INTO handoff_mysql VALUES (1,30)`；M Prepare `SELECT v FROM handoff_mysql WHERE id=?`/Execute整数1；M BEGIN、`UPDATE handoff_mysql SET v=31 WHERE id=1`，交错上行PG拒绝/恢复，M/N Query `SELECT id,v FROM handoff_mysql ORDER BY id`，M再次原Execute，M ROLLBACK，双方Query/原Execute | 初始prepared为30；M事务内Query与prepared为31、N为30；PG恢复不改变MySQL库/模式/事务；M回滚后双方30。按MySQL自身参考采值/NULL/列type/flags/charset/警告/错误，不能套PG OID或失败态规则。 |

每例独立重置夹具，先记录双端版本、配置和setup结果，准备失败即登记能力门槛。错误后先Sync（扩展组）、完整ROLLBACK并确认I，再关闭存在的statement/portal/prepared；事务外按实际创建情况删除public.handoff_blocked、public.handoff_pg及MySQL handoff_mysql，关闭连接。清理删除不算回滚证据。首个错误、后续门禁及恢复分别留消息与S；成功SQLSTATE不适用，无结果列说明原因，其余未采字段显式标缺证。实际observations未采集、verdict未验证，有效运行用例数0。

本轮Ready按纯文档范围验章节结构、引用路径、来源映射、契约一致性和diff；`.agents/skills/tidb-verify-profile/SKILL.md` 缺失，不声称执行其内容。源码/独立测试静态阅读、官方规范目标及历史协议运行记录均与本轮零运行分开。正确性风险为错误对象/重复写入、事务前缀泄露与回滚残留；兼容性风险为PG目标与产品门禁、类型/诊断/绑定时点差异；性能未测试，无延迟、吞吐、锁等待或扩展性结论。

### 后续细分

#### 会话执行链与契约模板

按当前计划串行补充各窄契约块，并在这五个三级标题下追加自己的同名四级小节，保留本块和已交付正文。先以第 2–4 章契约定义每项 DML/CTE 的共享执行接入，再逐项交接名称、prepared、序列和事务状态；它们的目标验收应复用本模板及第 1 章证据记录，不重建第二套 ID/运行结论格式。

本块留下的后续验证入口均为**待新增**用例定义，不表示新增了测试文件：补齐完整值/元数据及两连接副作用断言；验证 Parse 后事务状态变化再 Execute、错误阶段及 Sync 恢复；分别采产品拒绝和 PG 参考行为；交错 MySQL 原 Query/Prepare/Execute。工作流 6 补真实目录/对象证据，工作流 7 补精确 wire 和生命周期证据；事务型 DDL 另立局部可行性原型，未通过前保留拒绝，不在本轮建设完整子系统。文档检查可完成本块，以上运行缺口不能据此升级为功能支持。

#### 返回子句执行契约

后续实施任务先沿现有共享会话/执行链定位单次写入的行镜像与固定输出类型，再按 INSERT 默认值、UPDATE 新值、DELETE 旧值、零行/多行逐项开放；每项以本块独立测试定义及写后/提交/回滚证据为门槛。先扩充现有 [pg_dml_test.rs](../pkg/server/pg_dml_test.rs) 的非空表拒绝断言，再安排共享执行层对应独立测试；这些均为待新增测试意图，不表示本轮创建或执行了测试。不得只删除 command 门禁或改标签就宣布支持，也不预设新字段/API 或复制一套执行器。

工作流 2 提供三类 AST/投影保真与合法拒绝位置，3/4 提供结果表达式定型/求值，6 提供真实列/默认值和来源元数据，7 验证 Describe 不写入、错误传递、portal 完成/重复 Execute 不重复写入及输出格式；事务专项复用本块回滚证据并补状态门禁。OLD/NEW、identity、ON CONFLICT 与写入 CTE 分别等专项完成再组合。文档完成只关闭本块契约缺口，不能升级任何当前功能支持状态。

#### 冲突忽略执行契约

后续先沿第 2 章保真节点与真实唯一约束元数据接入共享 planner/执行链，逐项建立无目标、列目标、命名约束的独立测试，再证明多唯一键的非目标错误和混合批次原子性；部分索引推断单独开放，不以全局 ignore_errors 开关替代仲裁。优先扩充 pg_dml_test.rs 与共享执行层同目录独立测试，本轮未新增代码或测试文件。

RETURNING 前块提供固定投影与仅实际插入行的输出；下一块负责 DO UPDATE；目录工作流 6 提供真实约束/索引身份，协议工作流 7 采集 Q/扩展错误、恢复与精确 wire 元数据，事务专项复核失败状态和回滚。并发、排斥/延迟约束及 sequence 消耗交各自窄任务取证，保留未支持门禁。文档完成不代表 PG 冲突忽略已实现、18.6 差分通过或性能达标。

#### 冲突更新执行契约

后续沿保真冲突节点→真实仲裁元数据→共享旧行/候选行绑定与更新求值，分别建立普通更新、条件跳过、二次错误和重复命中独立测试；先扩充 pg_dml_test.rs 及同目录共享执行层测试，不只删除门禁或复制 MySQL 首冲突策略。本轮未创建测试文件或实现任务。并发/隔离证据另立窄验证，不为本块递归补全存储基础设施。

第 2 章交目标/两类 WHERE/别名及参数保真，第 3/4 章交类型与表达式，第 6 章交真实唯一约束/索引身份，RETURNING 前块交实际行与描述，第 7 章交 Parse/Describe 无写入、Execute/Sync/错误字段与生命周期，事务专项交失败态/回滚，第 8 章消费 MySQL 共存结果。文档完成仅关闭本题契约缺口，未证明功能已实现、18.6 通过或性能达标。

#### 只读公共表表达式契约

后续沿共享会话的查询入口核实实际分派，先补非递归绑定与定型、部分列别名、重复消费者及同目录独立测试，再由 server 两协议测试证明值/NULL/零行、错误状态与回滚。名称归第 2 章，列身份/codec 归第 3 章及工作流 7，纯表达式资格归第 4 章，真实对象和权限归工作流 6。复用现有 Rust CTE 实现，不因局部差异递归补建子系统。

递归与显式物化各另取 PG 参考和共享实现证据，覆盖调用次数、按需停止、取消/资源释放与优化约束后才能开放；volatile、sequence、写入 CTE 与主 DML 再单独组合。当前仅完成本块文档，未生成或执行代码任务，不升级任何支持状态。

#### 写入公共表表达式契约

后续先保留 WITH/嵌套写入/RETURNING 的完整表示与分析拒绝，再把 DELETE 行集生产者和主 INSERT 接入同一共享语句事务；快照及原子性未获证时保留整体拒绝。复用现有 CTE 绑定和返回契约，按搬移成功/零行、两侧失败与回滚、同快照/重复消费者三组窄证据分别细分，优先扩充 pg_dml_test.rs 和共享执行层同目录独立测试，不新增第二套执行器。未消费/无返回、主 SELECT、兄弟重复写入边界和递归组合分别设门槛，不借主搬移补全通用子系统。

来源步骤 1 的写入 CTE 由 Q 数据流及快照/次数规则承接，步骤 4 由成对失败、恢复及 MySQL 共存承接；名称与语法归工作流 2，固定类型归 3，表达式归 4，真实表/权限归 6，portal/错误与元数据归 7，双入口参考采证归 1/8，事务专项复用两表 COMMIT/ROLLBACK 证据。文档完成不表示功能开放；正确性及兼容性均未实测，性能无结论。本轮未生成或执行后续代码任务。

#### 模式与搜索路径契约

后续先细分路径原值/有效顺序与两连接隔离，再细分 SET/RESET 的事务提交/回滚恢复，最后单列 LOCAL 能力门槛与失败事务门禁，优先扩展 pg_session_test.rs、pg_name_test.rs、pg_query_test.rs 的独立测试。复用现有共享会话与实际元数据，不另建 PG 引擎或用字符串首项替代对象解析；先补两种协议的状态/值/副作用证据再开放能力。

来源步骤 2 的 schema/path 由 isolation/order/empty/missing 与生命周期用例承接，步骤 4 由 failed/local 门禁与 MySQL 共存承接。工作流 2 接有效路径的名称绑定，工作流 3/4 接类型/函数身份，工作流 6 提供 schema/权限/目录一致性，工作流 7 接协议状态，工作流 1/8 接参考采证；prepared 重绑定和事务型 DDL 各归专项。本轮只完成契约文档：正确性、兼容性未运行验证，性能无结论，不生成或执行代码实现任务。

#### 引号与大小写执行契约

后续仅拆三处窄交接：第 2 章精确名称身份传给共享 resolver；真实目录/存储身份与关系、列、别名的执行选择一致；两入口的结果列名及失败恢复证据。先以本块不同标记值证明读写对象与回滚，再决定最小命名适配及能力门禁方案，禁止直接放宽文本改写作为完成标准；优先扩展现有同目录独立测试，不在生产 Rust 文件内放测试。

工作流 2 负责词法保真及原位置，3/4 负责结果类型/表达式，6 负责真实目录、精确对象身份与权限，7 负责 Q/扩展错误与元数据，1/8 负责双参考端采证；路径复用前块、prepared 状态变化交下一专项，底层表示不足列为独立依赖。文档完成仅关闭来源步骤 2 的本题契约缺口，不表示 AsterSQL 已实现 PG18 或精确 18.6 兼容；本轮不生成/执行代码任务。

#### 预处理执行时会话契约

后续只交接三处窄边界：保留普通prepared的PG名称环境/原始参数身份供共享准备入口复核；依赖目录变更和路径变化时重新分析/拒绝并在副作用前校验固定结果类型；绑定portal与新Bind/新目录Execute分开管理。先验证stable/path-missing/qualified/drop-recreate/ddl-shape，再在多schema能力独立满足后验证path-switch/earlier-shadow，不为本题补建完整schema或缓存子系统。

工作流2交语法/名称绑定，3/4交固定参数/结果及函数身份，6交真实目录对象、变更可见性和依赖失效事实，7交Describe重验证、精确诊断、portal所有权/锁与Sync生命周期；1/8承接参考采证和MySQL自身基线。正确性风险为错误对象/旧行、重复执行及副作用前检查缺失；兼容性风险为重解析时点、结果形状/来源身份和错误恢复差异；性能未测、无结论。本块完成只关闭契约文档缺口，不表示功能实现或18.6实测通过，本轮不生成代码实施任务。

#### 序列执行契约

仅交接三个窄入口：共享序列分配/持久水位与PG is_called、回设和缓存边界；按真实序列ID保存连接currval并处理断连/错误/ROLLBACK；PG函数签名、regclass解析和权限分流。先完成CACHE 1的first/cross、rollback、setval true/false、边界与恢复证据，再单独验证CACHE>1、崩溃与多节点，不能递归扩写存储系统。

工作流2/3/4分别交语法名称、类型、函数注册，6交真实序列/默认值引用/owned-by，7交扩展阶段、副作用次数、诊断与Sync，1/8交参考采证及MySQL基线；identity后块复用本契约。正确性风险为回退序列水位、currval跨连接污染和重复求值；兼容性风险为原生权限、NULL/错误、setval回设及缓存差异；性能未测，无性能结论。本块只完成文档，不表示PG序列已支持或18.6差分通过。

#### 标识列生成契约

后续按“ALWAYS/BY DEFAULT和DEFAULT行生成”“SYSTEM/USER覆盖及拒绝”“执行/RETURNING与非回滚分配”“真实目录/拥有生命周期”“扩展与MySQL共存”分别实施，先复用共享列模型、规范会话与写入路径，不在本轮递归补建依赖。每块开放前须取得上方实际行、副作用、恢复和元数据证据；缺目录或序列前置时记录能力限制。

工作流2交语法/AST保真与拒绝位置，3交声明类型/NULL和结果身份，4交序列调用求值，6交identity属性、真实序列稳定引用、默认表达式与i拥有关系，7交准备阶段不分配、错误传递与Sync状态，1/8交参考端和双方言证据。正确性风险为错误回退计数器、重复分配、丢失行原子性及伪造默认引用；兼容性风险为把AUTO_INCREMENT/表达式生成列当identity、错误覆盖/NULL策略和错误诊断阶段；性能未测，无性能结论。本块完成仅表示文档契约交付，不表示identity功能或18.6差分通过。

#### 事务提交与回滚契约

1. 先在现有 pg_transaction_test.rs 的独立测试面补普通 Q 与 prepared 的完整值断言，以 commit、rollback、disconnect 的跨连接结果证明真实事务生命周期；共享引擎只补必要局部接线，不另建 PG 存储或执行器。
2. 单独取证 implicit、repeated-begin、outside-end；重复 BEGIN 不得泄漏旧事务写入。隐式扩展批处理与提交结果不确定的竞态另建窄任务，当前缺证不扩大本轮文档范围。
3. 将 error-recover 的共享事务/PG失败标志交批次13，Sync与显式/隐式事务界线交批次14，事务型DDL门槛交批次15；事务身份目录交工作流6，wire/连接清理屏障交7，MySQL共存回归交8。补丁18.6、崩溃持久性与隔离模式须有各自真实参考/存储证据后才能开放能力。

#### 失败事务恢复契约

1. 沿现有独立 pg_transaction_test.rs 补首错、门禁、完整回滚及跨连接完整值断言，单独覆盖提前Parse/Bind后进入E再Execute；只补共享执行链必需局部接线，不递归扩建事务系统。
2. 独立对齐失败态COMMIT：先采PG参考标签/状态/行值，再决定适配与测试迁移，保持实际回滚证据；另立保存点/ROLLBACK TO能力任务，验证E→T与局部保留，不复用完整ROLLBACK清标志逻辑冒充支持。
3. 交批次14扩展流水线同步与显式/隐式边界，交批次15事务型DDL门槛；错误码精化交第3/4章相关类型/表达式契约，wire字段交工作流7，共存交8。未实测PG18.6、输出阶段失败、回滚失败/取消竞态、真实存储故障及性能，不从本块文档完成推定支持或性能结论。

#### 协议同步与事务恢复边界

1. 先扩充现有独立pg_extended_test.rs与pg_transaction_test.rs的显式/隐式成功和错误段完整值断言，再确定共享会话最小提交/撤销接线；无BEGIN段的原子性不可从单条自动提交测试推导，不补建第二套事务引擎。
2. 分别取得Parse、Bind、Execute与有效取消的阶段/副作用证据，单列提前绑定portal与Q/扩展交叉门禁；工作流7负责受控取消屏障、消息顺序、对象生命周期及致命错误边界，避免将框架搭建塞入本题文档。
3. 来源步骤3由两种Sync序列与I/T/E分层承接，步骤4由前缀撤销、恢复新写和MySQL共存承接；批次13提供失败态出口，15接事务型DDL门槛，工作流1/8采参考与共存证据。正确性风险为部分写入提前提交、Sync误清显式E或旧portal绕门禁；兼容性风险为隐式边界、错误阶段及取消生命周期差异；性能未测，无结论。本块完成仅指文档，未证明PG18.6支持，不生成或执行代码任务。

#### 事务型建表能力门槛

来源步骤 3 的事务型 DDL 与不可伪造内部身份由最小rollback/commit探针、准入退出和MVCC/XID/WAL规则承接；步骤 4 的底层限制、成功/副作用、失败继续性和MySQL共存由reject/write-failure/database-restrict及共存用例承接。五节本块文档完成不表示可事务化。

后续仅交接普通建表的共享元数据/数据提交边界、双连接目录与名称可见性、错误/关闭后的资源清理三个窄调查面；先为真实工厂与独立测试补证，再另立遵循PLANS.md的原型实施计划。通过只可建议该子集实施，不通过写明底层限制，无法取证列具体环境/目录/故障屏障缺口；三种结局均不得自动删除当前门禁。DROP/ALTER/TRUNCATE及未列门禁命令逐项调查，数据库自身禁用项保留，完整DDL/存储子系统不纳入该原型。

工作流6承接真实对象/目录失效，7承接精确wire与关闭/取消屏障，1/8承接版本参考与MySQL共存；批次16汇总第5章交接，不把本块当运行通过。正确性风险是前缀隐式提交、对象先发布或回滚残留，兼容性风险是把产品拒绝与PG自身限制混同、目录与锁时序差异；均未运行验证。性能未测试，无吞吐、延迟、锁等待开销或扩展性结论。


#### 章节验收与后续交接

下表只是最小后续实施切口，不是本轮新建的任务文件或已存在API。2/3/4/6/7分别指语法名称、类型、函数、目录和wire工作流；1/8统一接收版本/证据与双方言回归。各切口复用本章对应族的完整验收，不因拆分而删除负例或副作用要求。

| 族 | 最小实施切口 | 必要交接依赖 |
| --- | --- | --- |
| RETURNING | 先补非空表拒绝证据，再分别接INSERT最终行、UPDATE新行、DELETE旧行和零行描述 | 2投影AST；3固定输出；4表达式；6真实默认/列身份；7 Describe无写入与一次Execute。 |
| DO NOTHING | 先列/约束目标仲裁，保留非目标错误及批次撤销 | 2目标保真；3/4候选值；6唯一约束/索引；7错误与实际插入行；组合RETURNING另验。 |
| DO UPDATE | 在仲裁基础上接旧行/候选行、条件及重复命中保护 | 2两类WHERE/别名；3/4定型求值；6仲裁；7次数/描述；并发独立验证。 |
| 只读CTE | 先绑定、部分列别名和重复消费者，经真实应用查询链取值 | 2作用域；3列身份；4按需求值资格；6对象；7两入口；递归/物化另验。 |
| 写入CTE | DELETE返回行到主INSERT的一个语句事务与共享快照 | 2完整写入节点；3/4行集；6两表；7一次生产/失败阶段；缺原子性保留整体拒绝。 |
| schema/path | 路径顺序与连接隔离，再SET/RESET事务恢复、LOCAL门槛 | 2有效路径；3/4类型/函数查找；6真实schema/权限；7设置入口与失败门禁。 |
| 引号/大小写 | 精确对象/列/别名身份穿透名称适配到真实读写 | 2词法/原位置；3/4结果身份；6后端可区分对象；7列名及诊断。 |
| prepared | 新Bind重分析与固定结果类型写前检查，旧portal独立管理 | 2名称环境；3/4固定签名；6依赖失效；7所有权/Describe/生命周期；多schema先满足独立门槛。 |
| sequence | CACHE1下真实序列ID、currval与setval is_called及非回滚 | 2/3 regclass/bigint；4函数签名；6真实序列/ACL；7执行次数；缓存/故障另验。 |
| identity | 模式/覆盖规则与实际行生成，再真实序列拥有生命周期 | 2属性AST；3声明类型；4分配；6列与序列i依赖；7准备不消耗；复用sequence/RETURNING门槛。 |
| 正常事务 | 先Q/prepared完整值和双连接提交/撤销，再断连清理屏障 | 2命令类别；3/4值/表达式；6真实行；7 I/T/E及关闭时序；隔离模式/持久性另证。 |
| 失败事务 | 先旧portal跨E门禁和完整回滚，再单独对齐失败态COMMIT | 2恢复命令分类；3/4首错；6副作用；7阶段/消息；保存点另立窄任务。 |
| Sync | 显式块与无BEGIN段分别证明成功前缀的提交/撤销 | 2/3/4提供各阶段输入/错误；6真实行；7丢弃/取消/Sync与portal；不从单条自动提交外推。 |
| 事务型DDL | 另立遵循PLANS.md的普通建表原型，先取得现有拒绝基线 | 2表定义；3/4列与表达式；6对象/目录发布；7超时/取消/关闭屏障；共享存储元数据/数据原子性缺证则保留拒绝。 |

实施前重新核实真实工厂链、最近独立测试、底层能力和源码冲突，按PLANS.md及适用验证规则另立实施计划。文档串行批次不是编码并行许可；重新评估Cargo共享槽位、执行资源和共享文件，不能按本文批次自动并行。优先扩充现有同目录独立测试，Rust生产文件与测试分离，只有必要局部接线进入当前切口，其他能力缺口交独立任务。

交给工作流6的是表/列/schema/唯一约束/序列/identity真实身份、权限与失效事实；交7的是Q/Parse/Bind/Describe/Execute、I/T/E、Sync、错误字段及取消/关闭生命周期；交8的是每族PG目标/产品限制与MySQL自身基线的关联用例。PG18.6精确参考构建、全链副作用、真实持久存储、DDL原型、并发/故障与性能均保留未取证入口。本章文档可验收完成，功能支持及上层来源任务状态不随之变更。

## 6. 系统目录与内省契约

### 模块

#### 目录执行链与证据模板

本块承接来源工作流 6 步骤 1、4，只建立目录执行边界与后续字段取证格式。批次 1 的依赖按实际正文核对：第 1 章提供能力 ID 与证据分级，第 2 章提供方言/名称边界（收口提交 `baaf4ccc07`），第 3 章提供固定类型及 Describe 契约（`4385f9d762`），第 4 章提供函数身份、工厂及求值边界（`00a70c3f5a`），第 5 章提供会话、事务与目录交接（`9b426e8c14`）。这些是已交付的文档契约，不表示功能已实现；来源编号任务的未开始状态不代替正文及提交证据，本轮不更新其状态。

以下为 2026-10-23 在源码基线 `9b426e8c14047d769b06a6d19f7e4d3a80517573` 经 RustCodeGraph status / 定点 node 阅读的静态证据；`pkg/server/doc.go` 不存在。本章不另建存储、优化器或通用执行器。

| 层与实际入口 | 当前事实、共享边界与限制 |
| --- | --- |
| 分类与绑定 | [pg_catalog_query.rs](../pkg/server/pg_catalog_query.rs)::parse / parse_shadowed 构造有界 Select/Expr；[pg_catalog.rs](../pkg/server/pg_catalog.rs)::CatalogQuery::parse_session / parse_session_with_types 取得会话 schema/path，检查隐式目录名是否被本地对象遮蔽，再 parameters / bind。返回 None 才进入普通 SQL 路径；已识别目录查询的语法/绑定错误不能用空集掩盖。bind 是目录表达式绑定，与协议 Bind 区分。 |
| 简单查询 | [pg_conn.rs](../pkg/server/pg_conn.rs) 的连接循环先尝试 SessionQuery，再 CatalogQuery::parse_session，经事务门禁与 PgService::with_query 调目录 execute；普通 SQL 经名称/语法适配调用共享 TiDBContext::execute_query。目录查询不是将完整 PG SQL 直接送给共享 planner。 |
| 扩展查询 | [pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::process 在 Parse 调 parse_session_with_types 并固定 metadata/parameter_oids；Bind 解码并 bind_values，portal 持有目录结构和值；Describe 读取描述。首次 Execute 经 pg_conn 提供的闭包调目录 execute，再 encode_formats；portal 缓存消息供后续分段发送。目录 PreparedMetadata::statement_id 为 0，不分配共享后端 prepared 句柄。 |
| 元数据与求值 | [conn.rs](../pkg/server/conn.rs)::TiDBContext::schema_snapshot 默认 None，真实 [runtime.rs](../pkg/server/runtime.rs)::ConcreteTiDBContext 实现返回 domain.info_schema()。CatalogQuery::execute 同时读取 SELECT DATABASE()，建立 Execution 的单次快照及 provider/CTE 缓存；execute_select 从该快照取值。有界求值现有上限为累计物化 16384 行、100000 次比较，带取消检查；这是当前实现约束，不是 PG 标准或性能承诺。 |
| 身份与编码 | [pg_oid.rs](../pkg/server/pg_oid.rs)::namespace_oid / table_oid / index_oid 统一检查原生 ID 的 OID 映射范围。CatalogQuery::metadata 生成 ColumnInfo/NativeType，execute 返回 QueryResult；[pg_result.rs](../pkg/server/pg_result.rs)::pg_type / write_result / encode_formats 转为 PG 描述及行。普通 SQL 的名称解析归第 2/5 章，类型归第 3 章，系统函数正式注册归第 4 章，wire/portal 生命周期交第 7 章；目录局部函数白名单不证明普通 SQL 已注册同名函数。 |

独立测试在 [lib.rs](../pkg/server/lib.rs) 用 cfg(test) 注册：[pg_catalog_query_test.rs](../pkg/server/pg_catalog_query_test.rs)::catalog_select_structure 覆盖目录 AST，catalog_projection_syntax_and_boundaries / catalog_binding_has_explicit_support 覆盖有界语法、字段及函数拒绝；[pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::catalog_projection_variants 含真实 domain 夹具、投影/类型/零行/错误恢复与 Parse 后元数据改变的断言意图。测试中客户端为手写 wire 夹具，不能由此推断某个 DataGrip 版本实际发过该 SQL。本轮均未运行。

#### 数据库目录映射

承接来源任务步骤 1、2，能力 ID `CAT-DATABASE/pg`，沿用前块证据模板。批次 1 的五节正文及提交 `946e9b2c206d913b73762990b551753fed204449` 已复核；累计第 1–5 章契约和该块所列交付提交存在，不由任务文件是否删除判断完成。以下为 2026-10-24 在该 SHA 使用 RustCodeGraph status、query、定点 node 阅读的静态证据，未运行测试；`pkg/server/doc.go` 不存在。

- [pg_catalog.rs](../pkg/server/pg_catalog.rs)::DATABASES_SQL 是仓库保存的数据库列表探针，输出 id/name/description/is_template/allow_connections/owner，当前库优先；模块注释称 DataGrip probes，但未采集 DataGrip 精确版本或官方生成 SQL，不能标为客户端实采。
- 同文件 CatalogQuery::execute 读取共享 schema_snapshot 与 `SELECT DATABASE()`；provider_rows 的 pg_database 分支直接枚举 snapshot.AllSchemas()，取 schema.id 与 schema.name.original，不按连接权限过滤，也未按 PG 可连接数据库种类筛选。原生系统 schema 是否应成为 PG 数据库仍是分类缺口，不能把原生全量列表宣称 PG 全量支持。[runtime.rs](../pkg/server/runtime.rs)::ConcreteTiDBContext::schema_snapshot 返回 domain.info_schema()，无第二套元数据存储。
- provider_rows 用 [pg_oid.rs](../pkg/server/pg_oid.rs)::namespace_oid(schema.id) 作为数据库行 OID；public namespace 也从当前原生数据库同一 ID 映射。它是受限映射，不证明 PG database/schema 是同一层；不同目录内 OID 相同本身也不表示对象相同。身份键须携带目录类别，pg_database 的系统关系 OID 1262 与用户数据库的行 OID 分开。
- CatalogQuery::column 将 oid 设 OID_TYPE，datname 为原生文本 code253，datdba 为 code8；provider_rows 的 datdba 为 SQL NULL，两个 bool 为固定 false/true。evaluate 的 pg_get_userbyid 无论输入均返回 NULL，current_database/current_catalog 返回上述共享当前库字符串。pg_shdescription provider 当前为空。

最近独立测试 [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::catalog_projection_variants 含真实 domain 数据库创建、OID 对照、datname text/OID25、bigint/OID20、零行描述、未知列 0A000 与 Parse 后首次 Execute 取新目录的断言；catalog_database_description_semantics 含空共享描述、LEFT JOIN 保行、f/t/NULL owner、不同启动数据库的 current_database/current_catalog 及 Parse 后删建数据库意图。它们由 [lib.rs](../pkg/server/lib.rs) 的 cfg(test) mod pg_catalog_test 注册，使用手写 wire 夹具；没有无 CONNECT 角色或真实 owner 的本轮运行证据。Q/扩展工厂分派、单次执行快照和有界求值复用前块，不另建目录执行器。

#### 命名空间目录映射

承接来源任务步骤 1、2，能力 `CAT-NAMESPACE/pg`。批次 2 的五节数据库目录正文及提交 `b7ff2288b8`、批次 1 的 `946e9b2c20` 和其累计第 1–5 章交付已核对，依赖满足。以下为 2026-10-24 在 `b7ff2288b8` 经 RustCodeGraph status、explore、query、定点 node 得到的静态证据；`pkg/server/doc.go` 不存在，未运行测试。

| 实际路径/符号 | 当前事实与证据边界 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::CatalogQuery::provider_rows / namespace_row | pg_namespace 从共享快照 AllSchemas() 中按 lower 名匹配 `SELECT DATABASE()` 的当前原生库，仅返回固定 pg_catalog（OID 11）及该库映射的 public；未找到当前库时仅剩 pg_catalog。不会枚举其他原生库为本库 schema，也没有用户 schema、information_schema、临时 namespace 的通用映射。namespace_row 的 owner 槽 5 为 NULL。 |
| 同文件 CatalogQuery::column / column_catalog_field | oid 使用 OID_TYPE；nspname 使用 code253，当前为 text/OID25；nspowner 使用 code8，为 bigint/OID20。nspacl 无字段绑定，落入 unsupported catalog column 的 0A000；不能把“未绑定”写成已返回 NULL ACL。xmin 的可空 bigint 适配不代表 PG 系统列语义，本块不扩充。 |
| [pg_oid.rs](../pkg/server/pg_oid.rs)::namespace_oid、[pg_catalog.rs](../pkg/server/pg_catalog.rs)::namespace_oid / class_row | public OID 为当前原生库持久 schema.id 的有界映射，0/越界拒绝；pg_class.relnamespace 使用同一入口。namespace 行 OID、pg_namespace 系统关系 OID 2615、数据库行 OID 分别带目录类别，不能以数值相等推断同一对象。用户 schema 尚无独立持久身份来源，不能用名称 hash 或 AllSchemas 的位置补造。 |
| [pg_session.rs](../pkg/server/pg_session.rs)::PgSession::{schema,public_precedes_catalog,execute} / SessionQuery::parse | schema 只取 path 首项，path 当前仅接受 public、pg_catalog 或空列表；任意用户 schema 返回 0A000。改变 path 影响名称选择，不改变 provider 的 schema 行集合。当前数据库仍属于共享连接，不能通过 SET search_path 切库。 |

简单 Q、扩展 Parse/Describe/Bind/Execute、ConcreteTiDBContext::schema_snapshot 的工厂接线和单次执行快照复用“目录执行链与证据模板”，本块不新增第二套元数据。[lib.rs](../pkg/server/lib.rs) 注册独立 [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_relations_live：真实 domain 夹具检查当前库表的 relnamespace、其他库 hidden 不出现，以及 public 行上的 regclass/遮蔽查询；这不是任意用户 schema 或权限证明。[pg_oid_test.rs](../pkg/server/pg_oid_test.rs)::pg_introspection_oid_ranges_and_local_indexes 检查 namespace/table/index 范围、无效 ID 与碰撞；不证明重启后的实际持久性。两者只读、均未运行，本轮有效运行数 0。

#### 关系身份与稳定标识

承接来源任务步骤 1、2，能力 `CAT-RELATION/pg`；批次 3 提交 `9bd191c61203ab5ba493a4261a0a89f0f33eda93` 的命名空间五节正文、累计批次 2 的 `b7ff2288b8`、批次 1 的 `946e9b2c20` 及其第 1–5 章交接已核对，依赖满足。以下为 2026-10-24 在该基线经 RustCodeGraph status/explore/query/定点 node 核对的静态事实；`pkg/server/doc.go` 不存在，测试未运行。

| 实际入口 | 事实来源与当前边界 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::provider_rows / class_rows / class_row | 单次共享快照中，固定 SYSTEM_RELATIONS 先生成 namespace=11、kind=r 的目录关系；当前原生库经 AllSchemas 查到 schema，再枚举 SchemaTableInfos。仅 StatePublic model 入列，View→v、Sequence→S、GetPartitionInfo→p、其余→r；表/视图/序列用 model.ID，namespace 用 schema.id。原生物理分区未作为独立 PG 关系，不能据 p 宣称完整 PG 分区语义。没有按当前角色过滤关系行。 |
| 同文件 catalog_indexes / column / column_catalog_field | 索引使用 (model.ID,index.ID)，StatePublic 才列行；当前按父表分区标记为 I/i。PKIsHandle 无显式主索引时按真实主键列构造 ID=-1、名称 PRIMARY 的适配身份，原生显式索引 ID<=0 则拒绝；这不是持久独立 PG 索引名称。oid/relnamespace 描述为26，relname/relkind 为 text/25，relowner 槽11为 NULL、int8/20。class_row 的其余占位值不证明对应完整语义。 |
| [pg_oid.rs](../pkg/server/pg_oid.rs)::SYSTEM_RELATIONS / namespace_oid / table_oid / index_oid | 固定目录关系如 pg_class=1259、pg_namespace=2615、pg_database=1262；用户 namespace/table/index 分离区间，计算不分配新 ID。非零有符号 ID 编码为 2×abs(id)−(id<0)，namespace 加16384且小于0x40000000，table 加0x40000000且小于0x80000000；index 对两编码作 Cantor 配对再加0x80000000且严格小于 u32::MAX，检查中间和及结果。ID=0/越界报错，不截断或 hash。固定系统 OID 不套用原生编码。 |
| 同文件 resolve_with_path / display；pg_catalog.rs::evaluate / project | cast 调 resolve_with_path，数字字符串可直接形成 u32，不要求存在；名称按引号保留/未引号折小写，仅支持当前库 public 与 pg_catalog，表优先于索引，重复索引名报歧义。public_first 控制局部遮蔽，display 无 path 参数，系统名直接返回、用户同系统名时加 public、未知 OID 返回数字。这不足以证明全部有效 search_path 下的往返。resolve 的候选过滤也未与 class_rows 的 StatePublic 全部对齐。 |
| [pg_name.rs](../pkg/server/pg_name.rs)::Resolver::relation / scope | 普通 SQL 名称重写保留 token/CTE 作用域，public 映射当前原生库，最后由共享解析器、对象查找及授权负责；不是 regclass 目录局部解析器，二者一致性是目标。Q/扩展工厂、快照及结果编码沿用“目录执行链与证据模板”，不另建执行器。 |

最近独立测试 [pg_oid_test.rs](../pkg/server/pg_oid_test.rs)::pg_introspection_oid_ranges_and_local_indexes 覆盖系统 OID 唯一、正负/非法 ID、跨表同局部索引 ID 与映射无碰撞；[pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_relations_live 覆盖真实 domain 的表/视图/序列/分区/索引分类、其他库隔离、Parse 后改名/删建刷新及 public.pg_class 遮蔽；[pg_catalog_query_test.rs](../pkg/server/pg_catalog_query_test.rs)::pg_introspection_oid_live_casts 覆盖26/2205描述、1259、最大 oid、名称错误与格式错误。由 [lib.rs](../pkg/server/lib.rs) 独立 cfg(test) 模块注册；它们是手写 wire 测试意图，未证明重启持久性、双连接稳定性、真实 psql 或权限结果。本轮有效运行数0。

#### 列目录与声明类型

承接来源任务步骤 1、2，能力 `CAT-ATTRIBUTE/pg`。批次 4 的关系身份五节正文及提交 `7ade409a9d`、累计批次 3/2/1 的 `9bd191c612` / `b7ff2288b8` / `946e9b2c20` 和第 1–5 章交接已复核，依赖满足。以下为 2026-10-24 在 `7ade409a9d` 经 RustCodeGraph status/explore/query/定点 node 阅读的静态证据；pkg/server 与 pkg/meta/model 均无 doc.go。本轮测试运行数 0。

| 实际路径/符号 | 事实与限制 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::column_catalog_rows / column_catalog_field | provider 复用前块共享快照及当前原生库，枚举 SchemaTableInfos 的 ModelMeta；表须 StatePublic，跳过 Sequence；列须 StatePublic 且非 Hidden。attrelid=table_oid(model.ID)，attnum=Offset+1，验证正数、int2 范围、列 ID>0 及位置不重复。非当前库列不列入，没有角色过滤。缺完整 ModelMeta 报错。没有索引属性、固定系统目录属性及序列属性的完整行来源，不能由 pg_class 有行推断 pg_attribute 完整。 |
| [column.rs](../pkg/meta/model/column.rs)::ColumnInfo | 持久 ID、Name.O、逻辑 Offset、State/Hidden、FieldType、GeneratedExprString/GeneratedStored、DefaultIsExpr/DefaultValue 是实际来源。结构未提供 PG 删除槽位/identity 模式及其序列关联；ID 不是 PG attnum。ChangingFieldType 是在线变更中间态，不可替代已发布声明。 |
| pg_catalog.rs::native_column_type / column_default / format_column_type | FieldType 类型/flags/charset/Flen/Decimal 生成 OID及typmod；numeric 为4+(p<<16)+s（当前仅p=1..65、s=0..p），字符为n+4，时间精度为0..6。bool flag 或 tinyint(1) 的 Bool 默认值用于现有 bool 识别，是身份缺口而非目标规则。enum/set/array 拒绝。column_default 对非空 GeneratedExprString 拒绝；因此空 attgenerated 不证明生成列支持。identity 总为空，attisdropped 总false；原生DROP删除列并重新整理逻辑位置，无PG墓碑。 |
| [pg_result.rs](../pkg/server/pg_result.rs)::pg_type / encode_formats | 目录 attrelid/atttypid 描述26，attnum为21，attname当前25，atttypmod为23，attnotnull/attisdropped为16，attidentity/attgenerated为内部char/18。普通字符串结果仍映射25、普通结果typmod=-1，不能据目录1043/typmod值推断 SELECT/Describe 已保存声明。两条链路须按第3章对齐。 |

Q/扩展 Parse、Describe、Bind、Execute 的工厂分派、单次快照和有界求值沿用“目录执行链与证据模板”。[lib.rs](../pkg/server/lib.rs) 注册独立 [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_columns_live：真实 domain 夹具断言 int/decimal/varchar/date/datetime、类型关联、空集描述、DROP/ADD 后 Offset 与 ID 不同、ALTER 后类型/NOT NULL/默认值刷新、已有 prepared 重执行刷新；当前 DROP 后 added 为位置5，不是 PG 保留删除槽位的证据。[pg_types_test.rs](../pkg/server/pg_types_test.rs)::catalog_char_and_acl_wire_types 断言字符cast为bpchar/1042及ACL NULL描述，并非内部char/18成功证据，common_type_oids_and_values 验证普通类型出口。均只读未执行，手写 wire 夹具不能证明真实客户端运行。

#### 内建类型目录映射

承接来源步骤1、2，能力`CAT-TYPE/pg`。批次5的列目录五节正文和提交`af67d135cf181d87bcb3ba0a7240d2395b4742c0`、累计批次4/3/2/1的`7ade409a9d` / `9bd191c612` / `b7ff2288b8` / `946e9b2c20`及第1–5章交接已核对，依赖满足。以下为2026-10-24在该基线经RustCodeGraph status/explore/query/定点node阅读的静态证据；pkg/server/doc.go不存在，本轮测试运行数0。

| 实际入口 | 当前事实及不能推导的能力 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::COLUMN_TYPES / column_catalog_rows | 固定18项：16/17/18/20/21/23/25/26/700/701/1009/1042/1043/1082/1083/1114/1700/114，保存OID、目录名、显示名、category；namespace固定11，typtype=b、typrelid/typbasetype/typndims=0、typtypmod=-1、typnotnull=false、typisdefined=true；typdefault/typowner/xmin为NULL，只有1009的typelem=25。之后仍遍历当前库公开模型并调用native_column_type校验，原生enum/set或不完整模型可令pg_type查询失败，不能由固定行表推断不依赖用户元数据。没有动态域、复合类型或枚举行。 |
| 同文件 column_catalog_field / native_column_type | 已绑定oid/typnamespace/typelem等OID列；typname当前text/25，typtype/typcategory为内部char/18，typowner当前bigint/20；typarray、typlen、typdelim、typbyval等无绑定，查询时0A000，不能说已返回0或NULL。FieldType映射沿前块；code245声明映射json/114，而pg_result普通出口拒绝该code。 |
| 同文件 format_column_type / evaluate | 格式化按同一COLUMN_TYPES查找；0返回“-”，表外OID返回“???”，NULL修饰符与-1在bpchar上分别为character/bpchar；字符、numeric带符号scale、time/timestamp有局部分支。这不能证明已识别全部PG有效OID、数组修饰符或域名；目录局部format_type与第4章普通SQL函数注册分开。 |
| [pg_result.rs](../pkg/server/pg_result.rs)::CatalogColumnType::wire_type / pg_type | 目录专用描述含int2[]1005、int4[]1007、int8[]1016、oid[]1028、text[]1009、int2vector22、oidvector30、regclass2205、aclitem[]1034；其中多数没有COLUMN_TYPES行。vector是独立目录类型，不能当普通一维数组。普通文本折为25、typmod未保真，沿第3章修复责任；目录行存在不保证参数、表达式或binary codec已开放。 |
| [pg_oid.rs](../pkg/server/pg_oid.rs)::SYSTEM_RELATIONS / OID_TYPE | pg_type系统关系OID为1247，OID字段自身类型为26；int4类型行OID23是固定PG身份，不经过table_oid或namespace_oid用户对象区间计算。 |

Q与扩展Parse/Describe/Bind/Execute工厂、共享快照、结果编码沿“目录执行链与证据模板”。[lib.rs](../pkg/server/lib.rs)以cfg(test)独立注册[pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_columns_live（真实domain列→类型join、numeric格式化、RetrieveDataTypes排序非空、public类型空集）和[pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values、catalog_char_and_acl_wire_types（普通描述/值与bpchar1042、ACL数组1034描述意图）。前者模板来自[postgresql-protocol-first-phase.md](postgresql-protocol-first-phase.md)，不是已采集的某版DataGrip运行记录；没有typarray闭环或完整类型目录的本轮运行证据。

#### 索引目录与定义反解

承接来源任务步骤1、2，能力ID `CAT-INDEX/pg`、`CAT-INDEXDEF/pg`，沿用首块证据模板。批次6交付 `1807f7800b7ffee693ea136a6ea2e9b472966035` 的内建类型五节正文，以及累计批次1–5提交 `946e9b2c206d913b73762990b551753fed204449`、`b7ff2288b8f97722c5c8ad3985cfdf56b06dae17`、`9bd191c61203ab5ba493a4261a0a89f0f33eda93`、`7ade409a9de6f53a3f60327bf24d2a5441907f46`、`af67d135cf181d87bcb3ba0a7240d2395b4742c0` 与正文已核对，满足本批次文档依赖；第2–5章的名称、声明类型、函数工厂及DDL门禁继续有效。以下为2026-10-24在该基线经RustCodeGraph status / explore / 定点node取得的静态证据；`pkg/server/doc.go`不存在，本轮没有运行测试。

- [pg_catalog.rs](../pkg/server/pg_catalog.rs)::catalog_indexes 复制 TableInfo.Indices，拒绝原生索引ID≤0；若PKIsHandle且没有Primary索引，则查找public、非Hidden且有主键标记的列，以局部ID=-1、名PRIMARY、Unique/Primary=true补一行。该投影有真实整数句柄依据，不是可任意制造的索引；缺主键列则拒绝。现代码取首个匹配列，尚不能据此证明异常多主键标记可被正确诊断。
- 同文件 index_constraint_rows 从执行快照的当前数据库 SchemaTableInfos 取 model_meta，跳过非public表、序列和视图，只枚举StatePublic索引。按IndexInfo.Columns顺序，以列名查public/非Hidden列，取Offset+1并校验IndexColumn.Offset一致及int2范围；不是按表列顺序重排。当前接受Tp=Invalid/Btree，拒绝空列、前缀、UseChangingType、多值、vector/inverted/fulltext及非空ConditionExprString；没有明确INCLUDE、PG排序/NULL顺序、opclass或表达式树的保真来源。
- [index.rs](../pkg/meta/model/index.rs)::IndexInfo/IndexColumn 与 [table.rs](../pkg/meta/model/table.rs)::TableInfo 是事实入口。Unique/Primary/State/Tp/ConditionExprString等不能由显示字符串反推；Invisible、Global、BackfillState存在但当前provider未据此建立PG状态等价证明。[pg_oid.rs](../pkg/server/pg_oid.rs)::index_oid 对(表ID,局部索引ID)作有界映射；pg_class provider与regclass查找均复用catalog_indexes，因此虚拟主索引应贯通同一OID，越界拒绝沿关系身份块。
- pg_index字段绑定见column；index_constraint_rows当前把indnatts=indnkeyatts、indnullsnotdistinct=false、indoption全0、indclass空vector、indcollation全0、indisvalid/indisready=true，indexprs/indpred为NULL。这些常量不能充当真实语义证据。pg_get_indexdef在expr_type接受单参数和三参数，evaluate复用provider或重新读取同快照，按OID查行21整定义/行22逐列名称；当前忽略pretty差异，未知OID及越界列返回NULL，整定义固定public/USING btree并对标识符双引号转义。有界目录白名单不证明普通SQL已通过第4章正式注册。

独立测试在 [lib.rs](../pkg/server/lib.rs) 注册。[pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_constraints_live 使用真实domain加原生DDL夹具，断言复合主键1 2、唯一列3、普通列2、定义、列号/越界NULL、向量OID22、PKIsHandle投影、引号、前缀拒绝与删除后空集；其prepared刷新直接断言的是外键约束，不能据此声称索引刷新已测。[pg_datagrip_test.rs](../pkg/server/pg_datagrip_test.rs)::pg_datagrip_ui_metadata_core 读取 [RetrieveIndexColumns.sql](../pkg/server/testdata/pg_datagrip/RetrieveIndexColumns.sql)，断言(note,id)的顺序2/1及零基vector下标；该文件是仓库存量模板，未取得DataGrip精确版本、官方查询出处或客户端实采，不能标作UI通过。

#### 约束目录与定义反解

承接来源步骤1、2，能力 `CAT-CONSTRAINT/pg`。批次7提交 `84fb929330` 的索引五节正文、累计批次6/5/4/3/2/1的 `1807f7800b` / `af67d135cf` / `7ade409a9d` / `9bd191c612` / `b7ff2288b8` / `946e9b2c20` 以及第1–5章交接正文和提交均已核对，依赖满足；不以任务文件消失判断完成。以下为2026-10-24在 `84fb929330` 经 RustCodeGraph status/explore/query/定点node 阅读的静态证据；pkg/server与pkg/meta/model均无doc.go。本轮测试运行数0。

| 实际路径/符号 | 当前事实与证据边界 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::provider_rows / index_constraint_rows | pg_constraint和pg_index共用当前原生库的共享快照，要求ModelMeta、StatePublic且非视图/序列。catalog_indexes包含PKIsHandle虚拟主索引；所有Primary或Unique索引均投影p/u约束，包括无法区分来源的独立唯一索引，约束oid直接复用index_oid，conindid为该索引。普通非唯一索引不生成约束，但在过滤前也被校验，坏索引可使约束查询失败。没有角色过滤。 |
| 同文件 index_constraint_rows 的FK分支 | 公开ForeignKeys的ID须正数、Cols/RefCols等长非空、RefSchema须为当前原生库；按原生表名找父表，以列原名查公开非隐藏列，Offset+1形成两侧数组。按父索引列顺序寻找公开Unique索引，conindid指向父索引、confrelid指向父表。oid=index_oid(子表ID,−2×FK.ID)，与正索引及虚拟−1分开，仍受OID范围门禁；没有独立分配器。父模型发布状态与方法等价性未在此完整校验，不能推断任意合法PG引用索引选择均支持。 |
| 同文件 column_catalog_field / expr_type / evaluate | conkey/confkey为int2[]/1005，conexclop为oid[]/1028，contype及动作码为内部char/18；conname当前text/25。conbin绑定text/25但行值NULL；xmin为可空int8/20适配，非真实PG事务标识。deferrable/deferred/noinherit固定false；convalidated、conenforced、confmatchtype等没有字段绑定。反解绑定一/二参数签名，读取同一provider或fallback扫描的slot21，pretty参数只求值不改变文本；NULL参数返回NULL、未找到OID返回NULL。这是目录局部白名单，不证明普通SELECT函数工厂已注册。 |
| [table.rs](../pkg/meta/model/table.rs)::ConstraintInfo / FKInfo | CHECK有ID、Name、Table、ConstraintCols、Enforced、InColumn、ExprString、State；公开CHECK当前使index_constraint_rows直接0A000，不伪造PG树。FK有ID、Name、RefSchema/RefTable、Cols/RefCols、OnUpdate/OnDelete、State、Version。两个结构均无PG deferrable/deferred/validated标记；StatePublic与FK.Version不等于验证完成证据。CHECK的Enforced不能代替Validated；索引Unique也不能证明约束来源。 |

Q/扩展Parse→Describe→Bind→Execute工厂、共享ConcreteTiDBContext快照与结果编码沿“目录执行链与证据模板”；表/索引身份沿关系及索引块、属性号沿列块，不新建执行器。[lib.rs](../pkg/server/lib.rs)独立注册 [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_constraints_live：真实domain夹具覆盖p/u、复合FK、父索引关联、1005/1028/18描述、删除FK后prepared重执行刷新及NULL/未知反解意图；无CHECK/延迟/验证状态的成功断言。独立 [pg_datagrip_test.rs](../pkg/server/pg_datagrip_test.rs)::pg_datagrip_structure读取 [1869280154.sql](../pkg/server/testdata/pg_datagrip/1869280154.sql)，断言唯一键{2,1}、索引关联、排除操作符NULL和展开空数组。模板18列及SQL是仓库证据，没有DataGrip版本/驱动/实际采集来源，不冒称官方客户端生成查询或本轮通过。

#### 默认表达式目录契约

承接来源任务步骤 1、2，能力 `CAT-DEFAULT/pg`。批次 8 约束块提交 `52f558f71f`、累计批次 7–1 的 `84fb929330` / `1807f7800b` / `af67d135cf` / `7ade409a9d` / `9bd191c612` / `b7ff2288b8` / `946e9b2c20` 的五节正文及第 1–5 章交接已核对，依赖满足。以下是 2026-10-24 在 `52f558f71f` 经 RustCodeGraph status/explore/定点 node 与补充源码阅读得到的静态证据；server、meta/model 均无 doc.go，测试运行数 0。

| 实际路径/符号 | 当前事实与缺口 |
| --- | --- |
| [column.rs](../pkg/meta/model/column.rs)::ColumnInfo / GetDefaultValue | 当前 DefaultValue 与 OriginDefaultValue 分开；前者配合 DefaultValueBit 返回当前值，后者不能用于 ALTER 后反解。DefaultIsExpr 区分表达式字符串，GeneratedExprString/GeneratedStored 保存原生生成表达式及存储方式，Dependences 是名称集合。Option 的 None 本身不能证明显式 DEFAULT NULL 与无 DEFAULT 可区别；没有 PG 表达式树或默认对象独立持久 OID。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::column_default | 生成表达式非空先拒绝；None 返回无默认。布尔只收 0/1/false/true，数值经有限 f64 检查后保留文本；bytea、非 UTF-8、非有限浮点、零日期拒绝。表达式仅允许 CURRENT_TIMESTAMP 与精度 0–6（包括原生引号精度归一），其余 DefaultIsExpr 拒绝；不能由白名单推定时区/事务时间语义等价。其他常量按类型名加引号和 cast，只转义单引号，反斜线与客户端编码仍需验证。 |
| 同文件 column_catalog_rows / column_catalog_field | 从当前库共享快照取公开表/列，按 column.Offset+1 形成 adnum，column.ID 用于模型身份校验；adrelid 沿 table_oid。仅 Some(default) 生成三槽行：adrelid/adnum/`relid:SQL片段`；adbin 当前是 text/25，目标 pg_node_tree/194 尚缺；没有 oid 字段绑定。pg_attribute.atthasdef 同取 default.is_some，attgenerated 为空。过滤前构造整库列行，其他表不支持的默认表达式也可能令查询拒绝，不能保证 WHERE 可隔离坏模型。 |
| 同文件 expr_type / evaluate | pg_get_expr 只绑定二参数（文本、数值），NULL 任一参数返回 NULL；非 NULL 以冒号拆 owner，匹配 relation 后返回后缀，未解析 PG nodeToString，也未校验伪造树。三参数 pretty 签名未接。无 Vars 常量配 relation=0 的 PG 用法当前仍因 owner 不符拒绝。 |
| [pg_sql.rs](../pkg/server/pg_sql.rs)::column_suffix | ALTER COLUMN 重建后缀保留当前默认值，拒绝生成列；DefaultIsExpr 或 CURRENT_TIMESTAMP 前缀字符串可直接进入原生 DDL。这是普通 SQL 适配，不能替代 PG 默认定义反解的语义验证。 |

Q/扩展工厂接线、单次共享快照及编码沿“目录执行链与证据模板”；OID/attnum 沿关系与列块，不另建执行器。[lib.rs](../pkg/server/lib.rs) 独立注册 [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_columns_live，含真实 domain 的属性 LEFT JOIN 默认定义、常量/时间默认、零行描述、NULL 输入、假表达式拒绝及 ALTER 后重读意图；pg_introspection_dependencies_live 包含序列引用默认值拒绝。这些是手写 wire 测试，均未运行，不是客户端采集或功能通过证明。

#### 视图定义内省契约

承接来源任务步骤 1、2，能力 `CAT-VIEWDEF/pg`。批次 9 提交 `0e0f892cc2` 与累计批次 8–1 的 `52f558f71f` / `84fb929330` / `1807f7800b` / `af67d135cf` / `7ade409a9d` / `9bd191c612` / `b7ff2288b8` / `946e9b2c20` 均有五节正文及提交证据，第 1–5 章交接沿首块，依赖满足。2026-10-24 在 `0e0f892cc2` 以 RustCodeGraph status/explore/定点 node 核对以下静态事实；server、meta/model 无 doc.go，本轮测试运行数 0。

| 实际路径/符号 | 当前事实与边界 |
| --- | --- |
| [table.rs](../pkg/meta/model/table.rs)::TableInfo.View / ViewInfo | 持久模型保存 SelectStmt（serde view_select）、Cols、CheckOption、Definer、Security、Algorithm。SelectStmt 是字符串，没有 PG 已绑定查询树或依赖身份；Definer 不能直接冒充 PG owner OID。普通视图与 MaterializedView 是不同字段。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::expr_type / evaluate | 目录局部绑定支持数值/NULL 的单参数及数值/NULL、bool/NULL 的双参数，返回原生 code253→text/25；wrap_column/int4 与旧 text 名称签名尚未绑定。执行先传播任一 NULL，从共享快照当前原生库枚举 StatePublic 模型，用 table_oid(model.ID) 匹配；View 存在才提取定义，非视图和未知 OID 均 NULL。pretty 非NULL时未参与格式化；无角色过滤、无通用 PG 反解或物化视图分支。 |
| [pg_name.rs](../pkg/server/pg_name.rs)::stored_view_select / source_tokens | 原生解析器要求恰一语句，SELECT/集合操作直接 trim；CREATE VIEW 用 native=true 的 token 字节区间找 VIEW 后非引号 AS，去尾分号及 LOCAL/CASCADED/省略模式的 CHECK OPTION。字符串/引号中的 AS 不当分隔，注释跳过但切片保留原文本；它不转换原生反引号、函数、schema 名和大小写。解析失败返回的 0A000 元组在 evaluate 被转为 ConnError::Session，不能直接断言 wire SQLSTATE 保留0A000。 |

工厂接线复用首块：pg_conn 简单查询、pg_extended 扩展查询到 CatalogQuery，ConcreteTiDBContext::schema_snapshot 到同一 infoschema；本块不增加执行器。独立 [pg_client_integration_test.rs](../pkg/server/pg_client_integration_test.rs)::pg_introspection_clients_stored_view_source 含引号 AS、字符串 CHECK OPTION、注释、两种 CHECK OPTION、裸 SELECT 和非法/多语句拒绝断言；run_clients 含视图来源查询的 OID26 参数、NULL/未知 OID、错误 pretty 与 text25 描述意图。[pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_relations_live 建真实视图并检查 relkind=v，不证明反解正确。[lib.rs](../pkg/server/lib.rs) 独立注册两文件，均未运行。

仓库 DATAGRIP_VIEW_SOURCES_SQL 注释标记 DataGrip 2025.1.3、2026-10-03 日志及 statement 1869279758；这属于仓库存档来源声明，本轮未核验原始日志或 UI，不能升级为当前客户端通过。官方客户端需求另见目标行为。

#### 序列身份与依赖契约

承接来源步骤 1、2，能力 `CAT-SEQUENCE-DEPEND/pg`。批次 10 的 `3ac33b4c83` 及累计批次 9–1 的 `0e0f892cc2`、`52f558f71f`、`84fb929330`、`1807f7800b`、`af67d135cf`、`7ade409a9d`、`9bd191c612`、`b7ff2288b8`、`946e9b2c20` 均有五节正文和提交，前置满足；第 1–5 章交接沿首块，特别引用第 5 章序列执行与标识列生成契约。以下为 2026-10-24 在 `3ac33b4c83` 经 RustCodeGraph status/explore/定点 node 核对的静态事实；server、meta/model 无 doc.go，测试未运行。

| 路径/符号 | 当前事实与证据限制 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::provider_rows / native_metadata_rows | pg_sequence 分支从本次共享 infoschema 快照当前原生库的 StatePublic TableInfo.Sequence 取参数；seqrelid=table_oid(ID)，seqtypid 固定 20，Cache=false 时 seqcache=1，否则 CacheValue；不调用序列分配。非序列无行，AUTO_INCREMENT 不新增序列行。 |
| 同文件 sequence_dependency_rows / column / column_catalog_rows | pg_depend 仅投影真实序列到命名空间的 n 边，classid=1259、refclassid=2615、两个 subid=0；缺当前 schema 时空集，缺完整 model 时错误。objsubid 虽在内部行末，字段绑定仍为 int4；attidentity 当前为空。全文未找到 pg_get_serial_sequence 分支，不能宣称该函数已支持。 |
| [table.rs](../pkg/meta/model/table.rs)::SequenceInfo；[column.rs](../pkg/meta/model/column.rs)::ColumnInfo | SequenceInfo 保存 Start/Increment/MinValue/MaxValue/Cache/CacheValue/Cycle/Comment，没有所属表列或声明序列类型；ColumnInfo 无 identity 模式及稳定序列引用。普通默认表达式也不能据文本构造已解析的序列依赖。 |
| [pg_oid.rs](../pkg/server/pg_oid.rs)::SYSTEM_RELATIONS / table_oid / namespace_oid | 固定系统关系 pg_class=1259、pg_namespace=2615、pg_sequence=2224；真实对象 OID 使用持久 ID 的检查映射，序列行和依赖 objid 复用 pg_class 的 OID，范围溢出拒绝，不截断。 |

工厂接线沿首块 pg_conn/pg_extended→CatalogQuery→ConcreteTiDBContext::schema_snapshot，无新增执行链。独立 [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_dependencies_live 使用真实 domain 建序列和 AUTO_INCREMENT 表，断言默认序列引用创建拒绝、列拥有查询空集但保留 26/26/23 描述、命名空间 n 边非空及 26/23/18 类型、其他库隔离、NULL/未知 OID、0A000 拒绝、Parse 后删建更新；是测试意图，不是本轮通过。[pg_datagrip_test.rs](../pkg/server/pg_datagrip_test.rs)::context/execute/pg_datagrip_tables 使用 CatalogQuery 手工夹具，未找到该文件的序列依赖专项测试。两文件由 [lib.rs](../pkg/server/lib.rs) 独立注册。依赖测试注释保存原 statement 1869279760，但未核验客户端原始日志/版本，不能把它升级为真实 DataGrip 本轮验收。

#### 函数目录与注册边界

承接来源步骤 1、2、4，能力 `CAT-PROC-REGISTRY/pg`。批次 11 的 `4dceaaab81` 与累计批次 1–10 的 `946e9b2c20`、`b7ff2288b8`、`9bd191c612`、`7ade409a9d`、`af67d135cf`、`1807f7800b`、`84fb929330`、`52f558f71f`、`0e0f892cc2`、`3ac33b4c83` 提交及五节正文均已核对。沿第 4 章三层身份、注册工厂、重载和必需系统函数契约；不将目录可查询升级为普通 SQL 可调用。2026-10-24 在 `4dceaaab81` 用 RustCodeGraph status/explore/定点 node 静态读取以下源码；server/expression 均无 doc.go，测试未运行。

| 真实入口/符号 | 当前事实与证据限制 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::FUNCTION_INTROSPECTION / function_rows / provider_rows | 仅三个内省原语 pg_get_function_arguments/result/sqlbody，适配器对象 OID 为16000/16001/16002，namespace固定11、language固定12、参数向量固定26、返回25。不是 PostgreSQL 官方函数对象 OID，也不是共享 expression 注册表镜像；public 不虚造用户函数。 |
| 同文件 column_catalog_field / expr_type / evaluate | proargtypes=OidVector、prorettype/pronamespace=oid，proacl 位于24，由执行行 resize 补NULL并绑定 AclArray；没有真实ACL来源。expr_type 按白名单及参数数量/数值类检查，evaluate 在静态表查OID，NULL输入返回NULL，未知OID拒绝，sqlbody为NULL。声明一个oid参数不等于当前验证已执行精确PG签名选择。 |
| [pg_session.rs](../pkg/server/pg_session.rs)::public_precedes_catalog；[scalar_function.rs](../pkg/expression/scalar_function.rs)::newFunctionImpl；[builtin.rs](../pkg/expression/builtin.rs)::invoke_factory | 前者只比较会话路径顺序；普通标量构造查 funcs/extensionFuncs、调用类工厂并读取返回类型，缺工厂明确失败。以上没有证明目录静态表已接入普通SQL名称解析或按 namespace/精确参数OID选重载，责任沿第4章。 |
| [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_functions_live；[pg_catalog_query_test.rs](../pkg/server/pg_catalog_query_test.rs)::catalog_select_structure / catalog_projection_syntax_and_boundaries | 前者以真实domain及手写wire验证三行internal函数、函数语言JOIN、NULL/未知OID/错误参数/未知名拒绝及恢复、来源SQL排除internal后空集、$1/OID26绑定。后者检查目录解析及边界，不能证明函数普通SQL可执行。均为读取断言意图，非本轮通过。 |

工厂接线与一次执行快照沿首块，独立测试由 [lib.rs](../pkg/server/lib.rs) 注册。测试保存的SQL自称 frozen JDBC，未核验驱动版本及上游原始SQL，不当作真实JDBC/DataGrip验收。

#### 对象注释与描述契约

承接来源步骤1、2，能力 `CAT-COMMENT/pg`，沿首块证据模板。批次12提交 `434724ad84`、批次11 `4dceaaab81` 与累计批次1–10的提交及五节正文已核对（提交链见前块）；第1–5章证据、名称、类型、函数工厂与会话契约继续适用。以下为2026-10-24在 `434724ad84` 使用RustCodeGraph status/explore/定点node的静态证据，未运行测试；server/model均无doc.go。

| 实际入口/符号 | 当前来源、接线与限制 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::provider_rows / native_metadata_rows | pg_description读取一次Execution快照的当前库SchemaTableInfos及model_meta，只取StatePublic对象。表取TableInfo.Comment，实际序列优先SequenceInfo.Comment；列取ColumnInfo.Comment及Offset+1，索引取IndexInfo.Comment及index_oid。非空字符串才产生行，classoid固定1259，表/序列/索引objsubid=0。隐藏列未另行排除，Offset不证明PG稳定属性槽位。缺完整模型拒绝。pg_shdescription当前空集。 |
| [table.rs](../pkg/meta/model/table.rs)::TableInfo；[column.rs](../pkg/meta/model/column.rs)::ColumnInfo；[db.rs](../pkg/meta/model/db.rs)::DBInfo；[infoschema.rs](../pkg/infoschema/infoschema.rs)::TableInfo | 前两者有Comment字符串，DBInfo没有数据库注释字段。精简缓存不含注释，须从model_meta取得完整模型；数据库名称或表注释不能充当共享描述。 |
| pg_catalog.rs::description_column / CatalogQuery::expr_type | objoid/classoid当前code8即int8/OID20，objsubid=int4/23，description=text/25。obj_description、col_description、shobj_description均未进入目录函数白名单，识别的目录表达式绑定拒绝0A000；不能据此推断普通SQL工厂同名函数可用。 |
| [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::catalog_database_description_semantics / pg_introspection_joins_live | 含无注释夹具零行、LEFT JOIN保行NULL、当前int8描述、Parse后数据库删建、共享描述无objsubid拒绝与恢复、多JOIN断言意图。没有由此证明真实注释写入、Unicode、权限或注释函数已通过。[lib.rs](../pkg/server/lib.rs)注册独立测试，本轮只读。 |

简单/扩展查询工厂、snapshot、OID检查与编码沿首块。数据库目录块中的pg_description空集证据是其历史基线，本轮native_metadata_rows已有非空来源；在此记录差异，不改其他块。

#### 角色权限与所有权契约

承接来源步骤1–3，能力 `CAT-ROLE-ACL/pg`，沿首块证据模板。批次13提交 `39300031ef`、批次12 `434724ad84`、批次11 `4dceaaab81` 及累计批次1–10的连续提交与五节正文已复核；第1–5章的证据分级、名称、类型、函数工厂及会话契约继续适用。以下为2026-10-24在 `39300031ef` 经RustCodeGraph status/explore/定点node读取的静态证据；server及privilege目标目录没有doc.go，本轮没有运行测试。

| 实际入口/符号 | 当前来源、接线与证据限制 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::provider_rows / CatalogQuery::column | pg_user执行共享 `SELECT User, Super_priv FROM mysql.user`，只产生usename和usesuper；按两值去重，未保留Host，不能证明同名不同host合并为一个PG角色安全。usename当前text/25，usesuper为bool/16；MySQL SUPER不是PG超级用户等价证据。pg_roles、pg_authid、pg_auth_members未提供角色关系适配。 |
| 同文件expr_type / evaluate | current_user由TiDBContext::user_identity字符串在第一个@截断，当前text/25；不是有效PG角色切换的证明。pg_get_userbyid仅接受NULL或当前code8表达式，输出text/25且固定NULL；has_*_privilege与pg_has_role未进入此目录函数白名单。识别的目录表达式绑定缺能力拒绝，不能推断普通SQL工厂已注册。 |
| [conn.rs](../pkg/server/conn.rs)::TiDBContext::authenticate；[runtime.rs](../pkg/server/runtime.rs)::ConcreteTiDBContext::authenticate / user_identity | 接口区分认证和用户身份；具体bootstrap认证只在InsecureRootOnly允许空认证数据的root/native-password，SecureUnsupported拒绝；成功发送SetAuthenticatedUser并标PROCESS。身份经SessionRequest::UserIdentity取得。该路径不能证明任意用户认证、PG角色授权或owner事实。协议认证目标交第7章。 |
| [privilege.rs](../pkg/privilege/privilege.rs)::Manager | ConnectionVerification/MatchIdentity处理原生user/host；RequestVerification、RequestVerificationWithUser处理库/表/列静态权限；ActiveRoles与RequestDynamicVerification另有角色/动态权限入口。它们是复用候选，当前目录求值没有接入PG角色OID、PUBLIC、owner及PG权限映射的证据，不能把接口存在标成已完成接线。 |
| 独立测试 | [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::catalog_database_description_semantics断言未知owner探针为NULL（现状，不是目标）；[pg_client_integration_test.rs](../pkg/server/pg_client_integration_test.rs)的namespace/database列表断言owner为NULL、tablespace为空。测试为仓库脚本/夹具，非DataGrip版本实采；[lib.rs](../pkg/server/lib.rs)注册测试。本轮未运行，没有最小权限角色、GRANT/REVOKE、owner切换或真实客户端结果。 |

目录Q/扩展分派、单次Execution快照、OID和wire沿首块；关系、数据库、namespace、类型、函数owner/ACL字段沿对应块，只在此定义共同安全事实来源，避免另一套角色或授权存储。

#### 可见性与名称遮蔽契约

承接来源步骤2、4，能力 `CAT-VISIBILITY/pg`。批次14提交 `9263b10237`，累计13–1为 `39300031ef`、`434724ad84`、`4dceaaab81`、`3ac33b4c83`、`0e0f892cc2`、`52f558f71f`、`84fb929330`、`1807f7800b`、`af67d135cf`、`7ade409a9d`、`9bd191c612`、`b7ff2288b8`、`946e9b2c20`，对应五节正文已核对，依赖满足。以下为2026-10-24静态阅读，未执行测试；`pkg/server/doc.go`不存在。

| 实际来源/符号 | 现状与责任 |
| --- | --- |
| [pg_session.rs](../pkg/server/pg_session.rs)::PgSession::{public_precedes_catalog,has_public,schema} / SessionQuery::parse | 默认public；省略pg_catalog时视为目录优先，显式public在目录前才返回true。只接受public/pg_catalog及空路径，schema取首项，未按真实存在性/USAGE计算有效路径。状态生命周期沿第5章。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::CatalogQuery::parse_session_with_types / evaluate | 前置public时从当前库schema_snapshot取同名用户表，交[pg_catalog_query.rs](../pkg/server/pg_catalog_query.rs)::implicit_relations / parse_shadowed避免隐式目录截获；显式pg_catalog仍走目录。evaluate与类型白名单无pg_table/type/function_is_visible分支，不能用现有pg_class行当可见性实现。 |
| [pg_oid.rs](../pkg/server/pg_oid.rs)::name_parts / resolve_with_path / display | name_parts保存引号大小写；resolve按public_first选用户关系或SYSTEM_RELATIONS，关联table_oid/index_oid。只传布尔而非完整路径，未传has_public，故不能证明路径排除public后的regclass一致性；display没有路径参数，不能保证遮蔽时输出限定名。 |
| [pg_name.rs](../pkg/server/pg_name.rs)::Resolver::relation / adapt；[pg_extended.rs](../pkg/server/pg_extended.rs)::process | 普通关系适配使用has_public，显式public改到当前原生库；Parse分别选择SessionQuery、CatalogQuery或普通适配。目录白名单、普通函数注册与名称绑定是不同入口，工厂与共享快照沿首块，不另建执行器。 |

独立测试只读：[pg_session_test.rs](../pkg/server/pg_session_test.rs)::pg_introspection_namespace_parser_boundaries；[pg_query_test.rs](../pkg/server/pg_query_test.rs)::pg_introspection_namespace_session_state（两连接、空路径、Parse后SET）；[pg_oid_test.rs](../pkg/server/pg_oid_test.rs)::pg_introspection_oid_ranges_and_local_indexes（身份范围）；[pg_catalog_query_test.rs](../pkg/server/pg_catalog_query_test.rs)::pg_introspection_oid_live_casts（1259、2205/26描述、名称错误、真实索引）。这些意图未证明权限/遮蔽全链路，本轮运行数0。

#### 标准模式表视图映射

承接来源步骤1、2，能力 `CAT-IS-TABLE-VIEW/pg`，沿用首块字段与证据模板。批次15提交 `784fca1216` 的五节可见性正文、批次1–14连续交付（`946e9b2c20` 至 `9263b10237`）及第1–5章交接已核对，依赖满足。以下为2026-10-24在该基线经RustCodeGraph status/explore/query/定点node核对的静态证据；未运行SQL或测试。server、infoschema、executor、meta/model目标目录的doc.go均不存在。

| 实际入口 | 当前事实及限制 |
| --- | --- |
| [pg_catalog_query.rs](../pkg/server/pg_catalog_query.rs)::is_catalog_relation / parse_shadowed；[pg_name.rs](../pkg/server/pg_name.rs)::Resolver::relation | 目录识别依据pg_catalog及已知pg关系，不含information_schema.schemata/tables/views；普通关系适配仅接受当前库public，显式information_schema落入非public关系0A000门禁。不能据原生SQL能访问标准模式宣称PG路由已支持；混合pg_catalog JOIN也不证明标准视图已绑定。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::provider_rows / class_rows | provider没有这三个PG标准视图分支；pg_namespace只有pg_catalog及当前原生库映射的public，class_rows枚举当前库SchemaTableInfos的StatePublic对象，View/Sequence/partition区分v/S/p，不做本块所需角色过滤。复用首块Q/扩展工厂、共享schema_snapshot、单次执行快照与pg_oid，不新建目录存储或执行器。 |
| [infoschema.rs](../pkg/infoschema/infoschema.rs)::SchemaTableInfos；[table.rs](../pkg/meta/model/table.rs)::ViewInfo | 原生库、表名与持久ID、State/View/Sequence等来自共享模型；ViewInfo提供SelectStmt、CheckOption、Definer/Security，但Definer不是已证实的PG owner，SelectStmt不是已验证的PG定义。一个原生库映射当前PG database下public，不将AllSchemas每项变成本库schema。 |
| [infoschema_reader.rs](../pkg/executor/infoschema_reader.rs)::setDataFromSchemata / setDataFromViews | 原生入口委托replace_rows(DataRequest)，不是PG目录provider。旁读同名Go方法可见原生catalog常量、字符集/排序规则和MySQL授权字段，与PG列集及视图定义owner过滤不同；不据Go方法推断Rust运行结果。 |

最近独立测试：[pg_query_test.rs](../pkg/server/pg_query_test.rs)的真实目录/事务夹具在原生会话读取information_schema.tidb_trx，并明确PG只暴露public/pg_catalog；[pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_functions_live仅在原生会话查询routines；catalog_projection_variants有真实domain、PG描述及投影意图。这些均不覆盖本块三个视图。独立模块由[lib.rs](../pkg/server/lib.rs)注册，本轮只读、实际运行数0，不将测试存在写为通过。

#### 标准模式列映射

承接来源步骤1、2，能力 `CAT-IS-COLUMNS/pg`，复用首块证据模板。批次16五节正文及提交 `c064d09bad`、批次1–15连续提交（`946e9b2c20` 至 `784fca1216`）和第1–5章公共契约已核对，依赖满足。以下为2026-10-24在该基线经RustCodeGraph status/explore/query/定点node核对的静态证据；server、infoschema、meta/model目录无doc.go，测试运行数0。

| 实际入口 | 当前事实与限制 |
| --- | --- |
| [pg_catalog_query.rs](../pkg/server/pg_catalog_query.rs)::is_catalog_relation；[pg_catalog.rs](../pkg/server/pg_catalog.rs)::provider_rows | 目录关系识别及provider没有information_schema.columns；继承标准模式表视图块的普通名称非public门禁。不能把原生MySQL标准模式列语义当作PG实现。Q/扩展工厂、共享schema_snapshot、单次执行快照与编码复用首块，不新建执行器。 |
| pg_catalog.rs::column_catalog_rows / native_column_type / column_default | 当前库SchemaTableInfos→公开非Sequence模型→公开非Hidden列；位置Offset+1，关系table_oid，声明OID/typmod由FieldType生成。无角色列过滤，无删除槽位保留；默认反解有白名单、生成表达式拒绝，identity空，非PG身份来源。映射细节沿列目录、内建类型及默认表达式块；这些函数尚未接成标准视图。 |
| [infoschema.rs](../pkg/infoschema/infoschema.rs)::InfoSchema；[column.rs](../pkg/meta/model/column.rs)::ColumnInfo | 共享模型提供Name、ID、Offset、State、Hidden、FieldType、DefaultValue/DefaultIsExpr、GeneratedExprString/GeneratedStored；当前默认与OriginDefaultValue不可互换。缺PG域、真实identity模式/序列关联、PG collation与完整可更新分析。原生AUTO_INCREMENT不能推导is_identity=YES。 |

独立 [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_columns_live含类型join、默认值、零行描述、ALTER/DROP/ADD和prepared重执行意图，并断言原生生成列拒绝及AUTO_INCREMENT的空attidentity。[pg_query_test.rs](../pkg/server/pg_query_test.rs)的PgCreateSimple夹具读 [RetrieveColumns.sql](../pkg/server/testdata/pg_datagrip/RetrieveColumns.sql)，验证15列PG声明经共享模型返回；该查询使用pg_catalog，不能证明标准columns视图或真实DataGrip运行。本轮均只读未执行。

#### 标准模式约束映射

承接来源步骤1、2，能力 `CAT-IS-CONSTRAINTS/pg`，沿首块证据模板及第1–5章名称、类型、函数、会话契约。已核对批次17提交 `01682dbdd6`、累计1–16提交（`946e9b2c20` 至 `c064d09bad`）与对应五节正文，依赖满足。2026-10-24在该基线用RustCodeGraph status/explore/query/定点node核对以下静态事实；server、infoschema、meta/model无doc.go，测试运行数0。

| 实际来源/入口 | 现状与证据限制 |
| --- | --- |
| [pg_catalog_query.rs](../pkg/server/pg_catalog_query.rs)::is_catalog_relation；[pg_catalog.rs](../pkg/server/pg_catalog.rs)::provider_rows | 四个information_schema视图尚未进入PG目录关系/provider；普通名称门禁沿标准模式表视图块。现有pg_constraint和pg_index共同进入index_constraint_rows，读取共享snapshot。Q/扩展工厂、Execution快照和wire沿首块，不能将原生MySQL标准视图视为PG接线。 |
| pg_catalog.rs::index_constraint_rows | 从当前库SchemaTableInfos/model_meta取StatePublic普通表；索引Primary/Unique转p/u，有混同独立唯一索引与约束的缺口。FK取有序Cols/RefCols、RefSchema/RefTable及动作，按严格同序寻找父唯一索引；CHECK存在即拒绝，未提供PG18 NOT NULL约束身份、真实延迟/验证/执行状态及PG授权过滤。不能直接复制成标准视图成功结果。 |
| [table.rs](../pkg/meta/model/table.rs)::FKInfo；[infoschema.rs](../pkg/infoschema/infoschema.rs)::InfoSchema | FKInfo有ID、Name、RefSchema、RefTable、Cols/RefCols、OnDelete/OnUpdate、State/Version；没有独立PG被引用约束身份与依赖边、MATCH/延迟字段。共享模型为候选事实源，PG OID、属性槽位、约束来源区分沿既有关系/约束块补证。 |
| 独立测试 | [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::pg_introspection_constraints_live读取真实domain的复合主键/FK、数组描述、动作和父索引JOIN，未覆盖这四个视图。[pg_datagrip_test.rs](../pkg/server/pg_datagrip_test.rs)::context/execute/pg_datagrip_tables使用CatalogQuery及仓库SQL模板验证pg_catalog；不证明标准约束视图或真实DataGrip版本通过。测试注册沿[lib.rs](../pkg/server/lib.rs)，本轮只读。 |

#### 无原生对象的响应策略

承接来源步骤3，能力CAT-ABSENT/pg，复用首块字段证据模板和第1–5章契约。已逐项核对批次1–18五节正文与连续提交`946e9b2c20`至`152374c1fe`，前置满足。2026-10-24用RustCodeGraph status/explore及定点node读取下列静态证据；pkg/server/doc.go不存在。本轮测试运行数0。

| 实际来源 | 当前事实与限制 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::provider_rows、column_catalog_field | tablespace、foreign_table/server、policy、trigger、rewrite、collation、opfamily/opclass/amop/amproc均返回Vec::new；不是PG参考库空集证据。foreign_data_wrapper尚无对应provider/字段映射，不能视为已知空关系。名称/owner多用253/8编码，部分字段缺失；零行仍可能描述错误。 |
| [pg_catalog_query.rs](../pkg/server/pg_catalog_query.rs)的目录分类/AST；CatalogQuery的metadata/bind/execute；[pg_result.rs](../pkg/server/pg_result.rs)::pg_type | Q、扩展工厂和单次快照沿首块；绑定应先于空行求值。253通常输出text/25，8输出int8/20，不等于name/19、oid/26；固定描述不可从零行推断。 |
| [pg_query_test.rs](../pkg/server/pg_query_test.rs)::tablespace_catalog_empty_relation | 手写wire夹具断言SELECT 0、两种投影及未知关系42P01/恢复，但spcname/owner/ACL/options期待25/20/25/25，不能作为PG类型正确的通过证据。[pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::catalog_projection_variants含LIMIT 0及未知列当前0A000，目标未知列42703须另验。均只读未运行。 |
| 仓库客户端模板 | [1869280143.sql](../pkg/server/testdata/pg_datagrip/1869280143.sql)请求FDW表/server/options；[1869280148.sql](../pkg/server/testdata/pg_datagrip/1869280148.sql)请求collation；[1869280149.sql](../pkg/server/testdata/pg_datagrip/1869280149.sql)、[1869280150.sql](../pkg/server/testdata/pg_datagrip/1869280150.sql)请求opclass/family；[1869280155.sql](../pkg/server/testdata/pg_datagrip/1869280155.sql)请求policy表达式；[1869280156.sql](../pkg/server/testdata/pg_datagrip/1869280156.sql)按classoid关联trigger/rewrite等注释。模板中的?是夹具占位，不宣称服务接受?参数；具体DataGrip版本和实际发出SQL未采集。 |

#### 目录求值与普通查询边界

承接来源步骤4，能力CAT-ROUTE/pg。已核对批次1–19的五节正文及连续提交`946e9b2c20`至`137d5d2894`；第1–5章依赖沿首块，前置文档交付满足。2026-10-24通过RustCodeGraph status与定点node核对源码；pkg/server/doc.go不存在，以下均为静态证据，测试运行数0。

| 实际入口 | 当前事实及证据边界 |
| --- | --- |
| [pg_catalog_query.rs](../pkg/server/pg_catalog_query.rs)::parse_shadowed、implicit_positions、Parser::query_inner/select/expr_inner | token关系引用分类，字符串/注释不是目录归属；未命中返回None。命中后只接有界只读SELECT，非目录relation不能混入；CTE有词法作用域。JOIN有INNER/LEFT/CROSS/NATURAL分支，ON绑定；不据此宣称RIGHT/FULL/USING或任意LATERAL支持。支持标量/IN子查询、有限数组展开及array_agg语法，不等于通用子查询/聚合。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::CatalogQuery::parse_session_with_types、bind、Execution::comparison/materialize | 会话path及共享schema_snapshot决定遮蔽，parameters/bind先确定类型；Execution共享单次快照、provider/CTE缓存、取消与累计预算。目录白名单函数不代表第4章普通函数工厂已注册。 |
| [pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::process/handle；[pg_conn.rs](../pkg/server/pg_conn.rs)连接循环 | Parse先SessionQuery再CatalogQuery，None才经pg_name/pg_sql进入共享prepare；目录metadata独立生成。Q经with_query选择目录execute或共享execute_query；扩展失败丢弃至Sync，并记录会话错误。Describe不能靠运行抽样补类型。 |
| 独立测试意图 | [pg_catalog_query_test.rs](../pkg/server/pg_catalog_query_test.rs)::catalog_projection_syntax_and_boundaries、catalog_binding_has_explicit_support、pg_introspection_joins_parameter_nodes、pg_introspection_cte_nesting_limits、pg_introspection_cte_column_bound、catalog_datagrip_scalar_and_lateral_bounds覆盖分类/绑定/边界；pg_introspection_oid_live_casts含wire OID描述与真实DDL身份变化意图。[pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync提供扩展协议夹具。只读未执行，测试存在不证明当前通过。 |

#### 目录更新与连接隔离

承接来源步骤4，能力CAT-REFRESH/pg。批次1–20的五节正文和连续提交946e9b2c20至7415cfbf46已核对；第1–5章依赖沿首块，前置文档交付满足。2026-10-24在7415cfbf46基线经RustCodeGraph status及定点node阅读，pkg/server/doc.go与pkg/infoschema/doc.go均不存在。以下为静态证据，独立测试运行数0。

| 实际入口 | 当前事实与证据限制 |
| --- | --- |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::CatalogQuery::execute / execute_select / execute_rows、Execution | execute先取得schema_snapshot，再由当前context执行SELECT DATABASE()，构造单次Execution；provider按关系名缓存，CTE按ID缓存，嵌套求值复用Execution。缓存每次execute重新建立。快照与数据库名分两步读取，尚不能据此证明并发删库/会话切换下原子一致，也不能推出事务历史目录快照。 |
| [runtime.rs](../pkg/server/runtime.rs)::ConcreteTiDBContext::schema_snapshot；[infoschema.rs](../pkg/infoschema/infoschema.rs)::InfoSchema；[cache.rs](../pkg/infoschema/cache.rs)::SchemaRef | 真实context取domain.info_schema()；InfoSchema提供SchemaMetaVersion、SchemaByName、TableByName、SchemaTableInfos。共享版本缓存不是连接目录结果缓存，当前入口没有按用户事务时间戳选择快照的证据。当前库来自连接context，不能用AllSchemas第一项替代。 |
| [pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::process | Parse保存目录结构和描述；首次Execute执行目录查询，portal.messages缓存编码结果，后续限行Execute续发已有消息。执行描述与portal.description不一致时在Execute返回0A000，消息尚未交给handle发送。新Bind/new portal与旧portal续读必须分别验收。当前schema会在首次Execute更新，不能据此推导所有名称绑定均重新解析。 |
| [pg_oid.rs](../pkg/server/pg_oid.rs)::table_oid / namespace_oid | 持久原生ID映射沿关系身份块；rename保身份，drop/recreate不得用旧名称缓存冒充旧对象。跨库身份不能只靠同名匹配。 |

独立测试只读未执行：[pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::catalog_projection_variants、catalog_database_description_semantics含Parse后建/删库；pg_introspection_relations_live含Parse后原生RENAME、新portal看DROP/CREATE；pg_introspection_columns_live含DROP/ADD、MODIFY后类型/非空/默认刷新及prepared重用意图。原生RENAME TABLE/MODIFY并非PG ALTER已支持的证明。[pg_transaction_test.rs](../pkg/server/pg_transaction_test.rs)::failed_transaction_requires_rollback断言事务内CREATE的0A000/E及ROLLBACK恢复；prepared_dml_commit_and_rollback只提供DML连接观察意图，不证明事务型DDL隔离。Q/扩展工厂连线沿首块，当前测试不涵盖下列全部竞争时序。

#### 命令行客户端内省流程

承接来源步骤 1、4，能力 CAT-CLI/pg。批次 21 的提交 `600fac8b1e` 与此前 `946e9b2c20` 至 `7415cfbf46` 的 20 个五节契约及第 1–5 章正文已核对，前置文档交付满足；不以来源任务仍写未开始或编号文件删除推断实现状态。2026-10-24 在 `600fac8b1e` 基线检查 RustCodeGraph status 并定点读取以下入口；pkg/server/doc.go 不存在。本轮独立测试运行数为 0。

| 客户端/入口 | 静态接线及最近独立测试意图；不是运行记录 |
| --- | --- |
| libpq | [pg_client_integration_test.rs](../pkg/server/pg_client_integration_test.rs)::pg_introspection_clients → run_clients(true)，以 CreateAnalyzeSession 的 domain、ConcreteSessionDriver::new_for_test(InsecureRootOnly)、CanonicalConnectionDomain 接入 Server::set_connection_runtime/run，Python ctypes 加载外部 libpq 执行 LIBPQ_WORKFLOW。PG_LIBPQ_LIBRARY 可选库路径，PQlibVersion 只断言 >=180000，并未固定精确补丁；循环协议 3.0/3.2，PQexec/PQexecParams 检查行/NULL/列名/PQftype，PQprepare/PQdescribePrepared/PQexecPrepared 检查 Parse 前建表前后行变化。服务声明 18.0 (AsterSQL) 不是参考 PG 版本。 |
| psql | 上述测试没有 Command::new(psql) 或反斜线元命令执行；不能把 libpq 或 DataGrip SQL 的通过断言当作 psql 的数据库列表、关系列表、表描述证据。精确 psql --version、-E 原始 SQL、输出与退出码均待采集。 |
| 共享目录及 wire 夹具 | [pg_catalog.rs](../pkg/server/pg_catalog.rs)::DATABASES_SQL 是现有数据库探针原文，CatalogQuery 与 Q/扩展工厂沿首块；[pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::catalog_projection_variants 经 PgService::start 和手写 query/columns 读取消息，不是真实 psql。[pg_oid.rs](../pkg/server/pg_oid.rs)::namespace_oid/table_oid/index_oid 从持久原生 ID 映射，不能按客户端返回行序分配 OID。 |

libpq 现有目录测试包含 DataGrip 来源查询、非空关系、类型正确空集、错误恢复及 prepared 建/删表闭环；其中 `live_catalog` 的 relname 描述断言为 text/25。它没有完整覆盖 psql 表描述、空用户库、真实非 owner 权限。JDBC 的 run_jdbc 与 DataGrip 查询归后续各自客户端块，不扩大本块范围。

#### 驱动元数据内省流程

承接来源步骤 1、4，能力 CAT-JDBC/pg。批次 22 提交 `868b3b138c`、累计批次 1–21 的五节契约（`946e9b2c20` 至 `600fac8b1e`）及第 1–5 章已有正文已核对，依赖是文档交付，不表示实现完成。2026-10-24 在 `868b3b138c` 基线经 RustCodeGraph status、explore 和定点 node 读取以下入口；`pkg/server/doc.go` 与 `.agents/skills/tidb-verify-profile/SKILL.md` 不存在，采用 Ready 文档替代检查。本轮测试运行数为 0。

| 实际入口/独立测试 | 当前证据与边界 |
| --- | --- |
| [pg_client_integration_test.rs](../pkg/server/pg_client_integration_test.rs)::pg_introspection_clients / run_clients / run_jdbc / JDBC_WORKFLOW | 真实 domain → ConcreteSessionDriver::new_for_test(InsecureRootOnly) → Server::set_connection_runtime/run；run_jdbc 读取 PG_JDBC_LIBRARIES，默认路径含 42.7.13、42.7.3，PG_JAVA 选择 Java。路径名不能证明实际 JAR 版本。工作流仅 getMetaData().getDriverVersion，以及手写 Statement/PreparedStatement、DataGrip 冻结 SQL、OID/NULL、DDL 刷新及错误恢复；没有本块七个 DatabaseMetaData 调用。本轮未启动 Java 或服务。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)::CatalogQuery::parse_session_with_types / bind / validate / metadata / execute | 首块的共享 schema_snapshot / infoschema/model 来源及 OID 映射继续适用；validate 限定关系/函数，不能从已有 DataGrip 查询推断驱动的窗口、集合返回、复合字段、正则、权限函数已覆盖。不能为一个驱动新增第二套目录事实。 |
| [pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::process；[pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync | Parse/Bind/Describe/Execute 工厂及参数、portal 编码沿首块和第 3/7 章；独立测试是手写 wire 夹具，不能证明驱动生成 SQL。 |
| [pg_catalog_test.rs](../pkg/server/pg_catalog_test.rs)::catalog_projection_variants | 使用真实 domain、PgService::start 和手写 wire，断言投影、NULL、描述及数据库真实身份；与上述扩展测试均只读取，未运行。 |

请求来源锁定为官方 pgJDBC **REL42.7.8** 的 PgDatabaseMetaData.java（下节链接），与本地测试默认 JAR 候选分开。本块有该版本源码证据，没有本轮该版本驱动运行证据；后续若改用其他版本，必须重新采集请求及版本分支。

### 目标行为

#### 目录执行链与证据模板

每项需求沿用第 1 章能力/用例 ID、分类、observations 和 evidence_refs，再加以下目录字段；同一查询的每个输出表达式分别有字段行，不能只登记关系名。已有静态实现、已有测试意图、当前运行记录三类分开保存，前两者不可填入“当前通过”。

| 必填项 | 内容与缺证规则 |
| --- | --- |
| 需求与客户端 | 能力/用例 ID、所属窄块、客户端及精确版本、驱动及版本、操作入口、官方出处及访问日期；人工 SQL 写“人工契约探针”，实际客户端生成 SQL 须附该版本抓取/官方源码，未抓取写未采集。 |
| 完整输入 | 原 SQL、逐位置参数值/类型/OID及格式、Q 或 Parse/Bind/Describe/Execute/Sync、结果格式、数据库/角色/编码/path、初始事务状态、准备/清理 SQL。无参数明确 []。 |
| 关系与字段 | 限定关系、别名、原字段/输出表达式和列序；关联键及其目标关系、对象身份与生命周期、NULL/零行含义。投影强转后的类型与原字段分列，不混淆数据中的对象 OID 和描述中的类型 OID。 |
| 类型与安全 | 每列 PG 类型及 OID、可空性、typlen/typmod与适用来源表/属性元数据、文本/二进制编码；权限前提、目录读取与对象使用权限、search_path 可见性、跨连接/跨数据库边界。未知属性写具体缺口，不由样本值猜类型。 |
| 真实事实来源 | 共享 infoschema/model/session 的实际路径/符号、原生对象 ID/版本及映射；没有来源逐项解释为空集、SQL NULL 或拒绝，不能造 owner、ACL、xmin、定义或序列值。SQL NULL 仅在目标语义允许时使用。 |
| 当前证据 | 源码 SHA/符号、独立测试路径/用例与断言范围；运行时间、命令/退出码、构建、完整版本、产物与有效用例数另记。无本轮运行填未验证，不以存在测试充当通过。 |
| 目标与不支持策略 | 目标值/行/类型、事实变化及隔离断言；缺来源/缺求值/缺 codec 分别登记拒绝阶段、目标 SQLSTATE 和副作用。成功 SQLSTATE 不适用；错误文本及具体协议字段未采则明示未采。 |
| 验收与交接 | 一条需求关联字段来源、目标和下节验收 ID；列出名称/类型/函数/会话/wire 协作及未验证范围。18.6 精确结论必须绑定参考构建，大版本手册不能替代。 |

目标依据（PostgreSQL 18，访问日期均 2026-10-23）：[系统目录](https://www.postgresql.org/docs/18/catalogs.html) 定义元数据角色；[pg_namespace](https://www.postgresql.org/docs/18/catalog-pg-namespace.html) 定义 namespace 字段及关联；[schema 与 search_path](https://www.postgresql.org/docs/18/ddl-schemas.html) 区分数据库、schema、名称查找和权限；[官方 REL_18_STABLE pg_type.dat](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/include/catalog/pg_type.dat) 用于核对类型身份，该分支可变，不是 18.6 构建证据。[psql 18 手册](https://www.postgresql.org/docs/18/app-psql.html) 的 -E/echo-hidden 是后续采集内省 SQL 的官方入口，不提供本示例的客户端查询归属。精确客户端版本和生成 SQL 仍须实采，JDBC/DataGrip/ORM 的实际查询留各自窄块取证。

#### 数据库目录映射

PG database 是连接所属数据库，schema 是库内命名空间；TiDB database/schema 的原生同义层仅作为当前数据库及其 public 的受限来源，不把所有原生库映射到同一 PG 库的 schema。目标 pg_database 枚举同一服务数据库集合，限定关系查询不受 search_path 遮蔽；改变 path 不改变列表或当前库，换库按第 5/7 章重新连接。目录 SELECT、CONNECT、认证/连接限制分别判断；datallowconn 不是当前角色权限位。下表各字段目录读取权限均为夹具显式具备 pg_database SELECT；拒绝 CONNECT 不应消除其目录行。

| 需求→字段/输出表达式 | 真实来源、关联键与当前证据 | PG 目标类型/值及缺失策略→验收 |
| --- | --- | --- |
| 数据库身份→N.oid；列表 id=N.oid::bigint | snapshot.AllSchemas() 的 schema.id 经 namespace_oid；pg_database.oid 为数据库身份键，描述需 (objoid=该键,classoid=1262)。现有 oid/OID26，强转 id 为 bigint/OID20。 | 原字段 oid/26、typlen4；id int8/20、8，非 NULL。两端用户 OID 不要求相等，各端同对象跨连接稳定。无快照/ID 越界应执行阶段拒绝 0A000，不造 0/顺序号或省略行；当前具体错误字段未实采。DB-BASE/FRESH。 |
| 名称→N.datname；列表 name | 同一对象 schema.name.original；当前 code253 描述为 text/25，测试有该断言，不是目标 name 类型。名称身份复用第 2 章，不能用展示小写覆盖带引号名称。 | name/19、非 NULL，标准 NAMEDATALEN=64 时 typlen64，参考构建仍需采证；缺 name codec 在输出前门禁 0A000，不静默降 text。DB-BASE/EMPTY。 |
| 所有者→N.datdba；列表 pg_get_userbyid(N.datdba) AS owner | 当前 slot5 为 NULL、code8→int8/20，局部函数也返回 NULL；没有真实 database owner 或 pg_authid.oid 关联来源。连接用户/mysql.user 不等于对象 owner。 | datdba oid/26、4、非 NULL，真实角色身份关联 pg_authid.oid；owner name/19，同库 owner 跨角色读取一致。未来需真实持久 owner 事实；缺失时含该字段/表达式的查询在结果输出前拒绝 0A000，不能将当前 NULL 当 PG 合法 owner，也不连带隐藏已有 oid/name 行。DB-OWNER。 |
| 模板属性→N.datistemplate；列表 is_template | 当前固定 false，无模板复制权限/生命周期元数据。 | bool/16、1、非 NULL。明确仅普通非模板数据库的受限集合可为 false；无模板功能不得虚构 template0/template1。请求模板身份/功能且无事实时在执行前门禁 0A000，不能给伪模板行；已有模板必须取真实值。DB-BASE/POLICY。 |
| 全局连接开关→N.datallowconn；列表 allow_connections | 当前固定 true，无 PG 每库允许连接开关来源，也无角色过滤。 | bool/16、1、非 NULL；仅在产品明示所有已映射普通库无禁用开关的受限契约下为 true。支持 ALTER 后须来自持久策略；不能用 has_database_privilege 的值替代，无可信策略来源且请求完整语义时输出前 0A000。DB-POLICY。 |
| 列表附属描述→D.description | 当前 pg_shdescription 空集，LEFT JOIN 保留数据库行、描述 NULL；完整共享对象键还含 classoid，原 DATABASES_SQL 仅按 objoid 关联，未来有其他共享对象描述时可能误连。 | text/25、-1，可 NULL；确无数据库注释功能时可保留类型正确空描述关系及 NULL，若已有注释须取事实，不能以空集掩盖。本块只验无注释初态及键约束，描述功能/安全 JOIN 补齐交后续窄块。DB-LIST。 |
| 当前库排序→CASE/current_database() | 当前来自同一共享连接 SELECT DATABASE()，不来自列表首行；现有局部函数为 text/25。 | current_database()/current_catalog 目标 name/19，非 NULL；当前库必须有对应真实数据库行，CASE 首组只该库，其余按 OID。不得在当前库缺失时随意挑首库；连接身份不可取得时输出前 0A000。DB-LIST。 |

以上 typmod 目标均 -1；bool 文本 t/f、OID 二进制四字节、int8 八字节、NULL 长度 -1 与 name codec 复用第 3 章；来源表/属性号、结果格式、完整 Describe 复用第 3/7 章并逐列采证，当前完整描述仍缺证。0A000 均为产品缺能力门禁目标，Q 在任何成功行输出前、扩展至迟首次 Execute 输出前拒绝；静态已可知的未开放字段/codec 可提前 Parse 拒绝，必须记录实际阶段，不能伪称 PG 参考端也如此。

官方依据（版本 PostgreSQL **18**，访问日期 **2026-10-24**）：[pg_database](https://www.postgresql.org/docs/18/catalog-pg-database.html)用于五个原字段的类型/含义与共享范围；[数据库层级](https://www.postgresql.org/docs/18/manage-ag-overview.html)用于库/schema/连接边界；[权限](https://www.postgresql.org/docs/18/ddl-priv.html)用于 CONNECT 与目录读取分离；[ALTER DATABASE](https://www.postgresql.org/docs/18/sql-alterdatabase.html)用于连接开关与模板属性变更；[系统信息函数](https://www.postgresql.org/docs/18/functions-info.html)用于 owner 名称和当前库函数身份；[pg_shdescription](https://www.postgresql.org/docs/18/catalog-pg-shdescription.html)用于复合对象键与可选描述。[psql 18 手册](https://www.postgresql.org/docs/18/app-psql.html)的数据库列表入口与 -E，以及官方 [REL_18_STABLE describe.c::listAllDbs](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/bin/psql/describe.c)说明 psql 列表还需求 encoding、locale、ACL 等；这条可变分支源码不代表精确客户端二进制，本块六列表 SQL 也不是 psql 完整列表查询。精确 PostgreSQL 18.6 构建、客户端版本及运行行为均未取证，不能由手册推定已通过。

#### 命名空间目录映射

完整目标区分 PG database 与库内 schema：当前原生 database→public 是受限桥接，不能把同服务的其他原生数据库列成本连接的 schema。一个 PG database 内须可存在 public、pg_catalog 和多个真实用户 schema；不同 database 可各有同名 schema，同一 database 的同名 namespace 不重复。系统和临时 namespace 仅在有对应真实语义时映射；完整枚举缺项必须登记能力缺口，不能宣称固定两行已完整兼容。

目录 SELECT 权限与 schema 的 USAGE/CREATE 分开；以下字段查询前提为角色具备 pg_namespace SELECT。无 USAGE 不应自动删除目录行，search_path 排除也不删除行；对象访问与有效路径过滤复用第 2/5 章。所有行都以当前数据库身份及 namespace 身份定位；pg_class.relnamespace、pg_type.typnamespace、pg_proc.pronamespace 后续须关联同一 pg_namespace.oid，不以字符串 public 或连接用户替代身份。

| 需求→字段 | 真实来源/关联键与当前证据 | PG 目标类型、NULL/不支持策略→验收 |
| --- | --- | --- |
| 身份→n.oid | 当前 public 来自上述 schema.id→pg_oid::namespace_oid，pg_catalog 为内建 11；目标须另有库内用户 schema 持久 ID。引用键为 pg_namespace.oid，跨库观察须带 database 上下文。 | oid/OID26、typlen4、非 NULL；同对象跨连接、改名稳定，删除后不得残留旧关联。无快照、缺当前库或超出映射范围在成功行输出前门禁 0A000，不以仅 pg_catalog、零行或 OID0 冒充成功。用户 OID 不要求两端数值相等，也不要求跨 PG database 数值全局唯一。NS-BASE/JOIN/FRESH/DB。 |
| 名称→n.nspname | 当前仅写固定 pg_catalog/public，非原生库名称；用户 schema 目标来自真实 namespace 名称及其所属库，不由 search_path 构造。 | name/OID19、非 NULL；标准 NAMEDATALEN=64 时 typlen64，构建值待采。引号大小写复用第 2/5 章。缺 name codec 时输出前 0A000，不能把当前 text/25 当目标已满足；不存在名称筛选才合法零行。NS-BASE/EMPTY/FRESH/DB。 |
| 所有者→n.nspowner | namespace_row 当前 NULL/code8；没有真实 schema owner→pg_authid.oid 来源，当前登录名、mysql.user 或原生 database ID 都不能替代。 | oid/OID26、4、非 NULL，关联真实角色。含本字段的查询缺 owner 来源须在输出前 0A000，不能固定超级用户或合法化当前 NULL；oid/name 的独立查询不因 owner 缺口而隐藏行。NS-OWNER。 |
| 授权→n.nspacl | 当前未绑定；未来须来自同一 schema 的持久 grantor/grantee/USAGE/CREATE/grant-option 事实及角色 OID，按 namespace OID 关联。 | aclitem[]/OID1034、typlen -1，可 NULL；仅真实默认授权状态可用 SQL NULL，显式空 ACL 与默认 NULL 不等价。缺 ACL 事实或文本 codec 的投影输出前 0A000，禁止伪造空数组/NULL/全员授权。数组元素 aclitem/OID1033，二进制能力另证，不能由其他数组 codec 推断。NS-ACL。 |

原字段 typmod 均 -1；列序、可空性、格式、来源关系 OID/属性号与完整 Describe 按第 3/7 章逐列采集，当前未取得完整 wire 描述。门禁 0A000 是产品未开放能力的目标：静态可知缺字段/codec 可在 Parse 拒绝，动态缺事实至迟首次 Execute/Q 输出成功行之前拒绝；记录实际阶段和原始错误，不冒称 PG 参考端也应拒绝。

依据（PostgreSQL **18**，访问 **2026-10-24**）：[pg_namespace](https://www.postgresql.org/docs/18/catalog-pg-namespace.html)用于四字段及 owner 关联；[Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html)用于库内命名空间和权限/路径边界；[Privileges](https://www.postgresql.org/docs/18/ddl-priv.html)用于 ACL 默认 NULL 与显式授权语义；官方 [REL_18_STABLE pg_type.dat](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/include/catalog/pg_type.dat)用于 name/oid/aclitem[] 类型身份。[psql 18 手册](https://www.postgresql.org/docs/18/app-psql.html)定义 `\dn[S+]` 与 `-E` 采集入口；官方 [describe.c::listSchemas](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/bin/psql/describe.c)显示名称/owner、verbose ACL/描述、系统对象过滤需求。源码分支可变，未固定客户端二进制；本块 SQL 均为人工探针，不是声称采集到的 psql 原始查询。精确 18.6 构建、真实客户端及其生成 SQL 均未取证。

#### 关系身份与稳定标识

沿用公共证据模板，区分关系行的 OID、系统目录关系自身的 OID 和输出列的类型 OID。对象键至少带数据库、目录类别和对象 OID；OID 不是跨集群全局标识，参考端与 AsterSQL 的用户 OID 不要求数值相等。下表要求目录读取角色具备 SELECT on pg_class/pg_namespace；目录可枚举、search_path 可见、对象可使用分别判断，无对象 SELECT 不应通过省略目录行来伪装拒绝。

| 需求→字段/表达式 | 真实来源、关联键与当前证据 | PG 目标类型/值、缺失策略→验收 |
| --- | --- | --- |
| 关系身份→c.oid | 表/视图/实际序列共用 table_oid(model.ID)；索引用 index_oid(父表ID,局部索引ID)，系统用 SYSTEM_RELATIONS。关联 pg_index.indrelid/indexrelid、pg_attribute.attrelid、pg_sequence.seqrelid 须指向同一 pg_class 行；这里只确定键，不接管其他目录字段。 | oid/26，typlen4，非 NULL；同一持久原生身份跨连接/服务重启/改名稳定，删除后旧行消失，同名新建使用新的持久身份/OID。原生 ID 保留是条件，不保证恢复导入或 ID 重用仍能区分；ID 重用须真实代际来源或门禁，不能承诺永不复用。缺快照/非法 ID/越界在任何成功行前拒绝0A000（产品目标），不丢行、取模或改顺序分配。REL-BASE/KEY/LIFE/RANGE。 |
| 名称→c.relname | 同一 model.Name.O/index.Name.O 或固定目录名；现为text/25。局部索引名可在不同原生表重复，PRIMARY 也可能重复，当前不能宣称 PG namespace 内所有关系名唯一。 | name/19，非 NULL，标准构建typlen64；目标同一库/schema 内表、视图、序列、索引共享关系命名空间。需要持久 PG 可见索引名及双向映射，不能用运行顺序或静默后缀解决；未解决重名时在创建冲突前门禁，既有歧义查找明确拒绝、不任选一个。缺 name codec 输出前0A000。REL-BASE/NAME。 |
| namespace外键→c.relnamespace | schema.id→namespace_oid，与前块 public 的 pg_namespace.oid 同源；系统关系固定11。 | oid/26，4，非 NULL；每行恰连本库一个真实 namespace，用户 schema 继承前块持久来源门槛；其他数据库不泄漏。限定目录查询不因 path 变化删行；缺真实 namespace 不造0。REL-BASE/KEY。 |
| 类别→c.relkind | StatePublic 的 View/Sequence/partition/Indices；当前值文本描述25。自增列不产生序列关系。 | 内部 "char"/18，1，非 NULL；r/v/S/i/p/I 必须分别有真实语义。尤其 p/I 需分区父对象及索引语义，原生分区存在不足以证明 PG 分区契约；不将物理分区 ID 当独立关系。无真实 m/t/c/f 等对象时无对应行；若请求创建此对象而未支持，在 DDL 副作用前0A000，不造 r 行。REL-BASE/KIND。 |
| 所有者→c.relowner | 当前 NULL/int8，缺持久 PG owner/pg_authid.oid 关联；不能从当前登录用户推造。 | oid/26，4，非 NULL，指向真实角色身份；ALTER OWNER 仅变 owner，不变关系 OID。无来源时请求该字段输出前0A000，不能把 NULL 或0当合法 owner。REL-OWNER。 |
| 名称解析/显示→text::regclass、oid::regclass、::oid | 共享 snapshot/model 与上述映射；当前局部 resolve/display，无完整 path 输出语义；不据此宣称普通 SQL 已有函数/cast 注册。 | regclass/2205，4；::oid 为26/4，显式::text 为25/-1，NULL传播。名称输入遵守第2/5章大小写、有效 path/USAGE，显示需能在该会话重新解析到同一对象，必要时限定/引号。未知数字 OID 可保留且显示十进制，不能反向造 pg_class 行；缺失名称 cast 失败，to_regclass(text) 对未找到名称返回 SQL NULL，未注册函数应门禁0A000而非假成功。REL-CAST/NAME/UNKNOWN。 |

上述 typmod 均-1，未强转的 name 与内部 "char" 不折成 text；文本/binary、来源表/属性号与零行 Describe 按第3/7章逐列采证。附属字段只登记当前值及门槛：xmin=NULL/int8（非 PG xid），reltablespace=0/oid、reloptions=NULL/text[]、relpersistence=p/内部char、relispartition=false/bool、relpartbound=NULL/text、relam=0/oid、relacl=NULL/aclitem[]；没有真实事务、存储、分区、ACL来源时不能把这些占位推为 PG 支持。请求其完整语义时输出前0A000；无显式选项/ACL的 NULL 仅在对应窄块证明语义成立后开放，relpartbound 目标 pg_node_tree/194而非text。其它未绑定字段沿公共拒绝契约。这里只要求不伪造，不在本块补齐 pg_class 全字段。

目标依据（PostgreSQL **18**，访问日期 **2026-10-24**）：[pg_class](https://www.postgresql.org/docs/18/catalog-pg-class.html)核对字段、关系种类与外键；[OID及其别名](https://www.postgresql.org/docs/18/datatype-oid.html)核对数字/符号、路径、引号及早绑定；[系统信息函数](https://www.postgresql.org/docs/18/functions-info.html)核对 to_regclass 缺名为NULL。[官方 REL_18_STABLE pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/include/catalog/pg_type.dat)核对19/18/26/2205/194类型身份，该分支可变，不是18.6运行构建。[psql 18](https://www.postgresql.org/docs/18/app-psql.html)的关系列表/描述与 -E 是客户端采集入口；本块 SQL 为人工契约探针，未声称 psql/JDBC/DataGrip/ORM 生成过它们。18.6精确行为、错误字段与客户端版本均未取证；产品0A000门槛不能冒称参考端错误。

#### 列目录与声明类型

沿公共证据模板，目录读取前提为角色具备 pg_attribute/pg_class/pg_type SELECT；对象可使用、列级权限、search_path 可见性分开判断。无表SELECT不应隐藏其列元数据；限定 pg_catalog 查询不受同名本地对象遮蔽；当前数据库隔离沿前块。以下字段均非 SQL NULL；属性主键是 `(attrelid,attnum)`，内部持久列 ID 另存映射，不以名称/向量下标重建历史槽位。

| 需求→字段 | 真实来源、关联与当前证据 | PG目标类型/值、缺失策略→验收 |
| --- | --- | --- |
| 所属关系→attrelid | model.ID→table_oid，与pg_class.oid同源；索引/系统/序列属性尚缺完整映射。 | oid/26、typlen4，关联恰一个本库pg_class行；不能造0。缺ID/快照或所请求关系种类的真实属性在成功行前0A000，不以空集伪装已覆盖。ATTR-BASE/KEY。 |
| 列序→attnum | 当前Offset+1，只证明原生逻辑位置。持久列ID不能直接强转为int2。 | int2/21、2；普通列从1开始，改名保持号，删除后幸存列不重编号，新增不复用删除号。系统列负号须有真实PG语义，不能把Hidden列直接变负数。无持久槽位来源时相关DDL副作用前门禁0A000，既有无法解释的状态执行拒绝。ATTR-LIFE。 |
| 列名→attname | 同一ColumnInfo.Name.O，当前text/25。 | name/19、标准构建typlen64；保留引号大小写。删除列名称为内部占位，验收不硬编码PG生成名称。缺name描述/codec输出前0A000，不静默降text。ATTR-BASE/LIFE。 |
| 声明类型→atttypid | FieldType→native_column_type；关联pg_type.oid。当前varchar与普通结果text身份分叉。 | oid/26、4；数据值为第3章规范声明OID，如int4=23、varchar=1043、numeric=1700；非由首行/NULL/SELECT表达式推断。删除列为0且不能inner join要求其有pg_type行；普通列不得为0。缺规范身份或映射拒绝0A000，不伪装text。ATTR-TYPE/KEY。 |
| 声明修饰符→atttypmod | 当前Flen/Decimal，仅在语义相符时可复用。不是字节显示宽度。 | int4/23、4；整数/text等无修饰为-1，varchar(n)/bpchar(n)=n+4，numeric按第3章带符号scale编码，时间显式p为p、未声明为-1。无约束numeric、负scale等不得强套MySQL界限；声明是否显式须真实保存。缺失时输出前0A000，不统一-1掩盖。ATTR-TYPE。 |
| 非空→attnotnull | 当前HasNotNullFlag(FieldType.flags)，须绑定已发布约束状态。 | bool/16、1；反映真实列NOT NULL，identity隐含true；不能由样本无NULL或已有数据推断。约束有效性遵守PG18语义，未支持相应约束状态须门禁而非伪报false。ATTR-NULL/IDENTITY。 |
| 删除标志→attisdropped | 当前固定false且删除行消失，无墓碑来源。 | bool/16、1；普通false，已删槽位true且atttypid=0；通常客户端筛attnum>0且NOT attisdropped。缺墓碑不能合成假行，相关生命周期按attnum门禁。ATTR-LIFE。 |
| identity模式→attidentity | ColumnInfo无模式与所拥有序列身份；当前空值，AUTO_INCREMENT不是identity。 | 内部“char”/18、1；非identity为零字节语义（文本显示空串），ALWAYS=a、BY DEFAULT=d；来源须含模式、表/列/序列真实关联，引用第5章标识列契约。不明模式不填空串或NULL；相关声明副作用前0A000，既有缺来源对象查询输出前拒绝。ATTR-IDENTITY。 |
| 生成列模式→attgenerated | GeneratedExprString + GeneratedStored可定位原生候选，但表达式PG等价性仍缺；当前候选提前拒绝。 | 内部“char”/18、1；普通零字节语义，stored=s、virtual=v；须真实表达式及相应读写行为才开放。MySQL表达式字符串不得直接冒充PG表达式；缺语义在DDL前或读取既有对象时0A000，不返回普通列。ATTR-GENERATED。 |

以上输出字段自身typmod均-1，atttypmod列中的数值是所描述列的修饰符，两者不同；text/binary、零行Describe及来源关系/属性元数据按第3/7章采证。`format_type(atttypid,atttypmod)`目标输出text/25，来自同一声明身份；它只呈现类型，不还原NOT NULL、identity、generated或整个DDL。atthasdef/pg_attrdef表达式交默认表达式窄块，identity与普通默认值的区别沿第5章；本块不扩写表达式反解。

官方目标依据（PostgreSQL **18**，访问日期 **2026-10-24**）：[pg_attribute](https://www.postgresql.org/docs/18/catalog-pg-attribute.html)用于字段、删除槽位、identity及生成模式；[字符类型](https://www.postgresql.org/docs/18/datatype-character.html)用于声明字符长度；[生成列](https://www.postgresql.org/docs/18/ddl-generated-columns.html)用于stored/virtual行为；[psql 18](https://www.postgresql.org/docs/18/app-psql.html)用于描述表及-E采集入口。类型数值/修饰符沿第3章已核官方REL_18_0 pg_type.dat、numeric.c、varchar.c，本轮另查[REL_18_STABLE varchar.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/utils/adt/varchar.c)与[numeric.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/utils/adt/numeric.c)核对编码。维护分支可变；18.6参考构建、错误协议字段及精确客户端版本未取证。下列SQL是人工契约探针，无psql/JDBC/DataGrip/ORM实采归属。

#### 内建类型目录映射

沿公共证据模板与第3章规范身份/typmod，目标为`pg_attribute.atttypid → pg_type.oid → format_type(atttypid,atttypmod)`使用同一身份。目录不是类型转换规则表：不在本块重定义coercion、隐式转换或codec。读取前提为目录SELECT；无用户表SELECT/无类型USAGE不应被误作内建类型目录隐藏规则，实际使用权限另验；限定pg_catalog不受public同名表遮蔽，未知/不可见名称与不存在OID分开。

| 需求→字段/表达式 | 真实来源、关联键与当前证据 | PG目标类型/OID及值、缺失策略→验收 |
| --- | --- | --- |
| 身份→oid / typname / typnamespace | 固定内建注册身份，当前COLUMN_TYPES；namespace关联pg_namespace.oid=11。用户类型须另有持久对象来源。 | oid/26、name/19、oid/26，标准构建typlen4/64/4；名字为int4、varchar等目录名，不能以integer等显示别名替代。所有已暴露声明及目录描述OID必须有恰一类型行；缺注册或name codec输出前0A000，不补text假行。TYPE-BASE/CLOSURE。 |
| 类别→typtype / typcategory / typisdefined | 当前固定b、表内category、true；目标取版本固定的类型定义。 | 内部char/18、内部char/18、bool/16，长度1/1/1；真实数组typtype仍b、category=A；vector不能仅凭typelem归类A。域d、复合c、枚举e等只在真实实现时开放，不能一概b。TYPE-BASE/ARRAY/DOMAIN。 |
| 容器关联→typelem / typarray / typdelim | 当前仅text[]→text，typarray/typdelim未绑定；目标按PG内建注册表及真实容器语义。 | oid/26、oid/26、内部char/18，长度4/4/1；int4(23)→typarray1007，1007→typelem23；text25→1009→25，例中delimiter为逗号。0仅表示语义上无关联；typelem非0不充分证明普通数组，如vector及name均需单独核对。不得按OID算术推导配对。TYPE-ARRAY/JDBC。 |
| 内部长度→typlen / typbyval | 当前未绑定；目标取PG参考构建定义，与自身引擎物理布局区分。 | int2/21、bool/16，长度2/1；int4为4，text/varchar/numeric及普通数组为-1，name标准构建64；typbyval依参考构建，不能从wire长度猜测。缺构建/注册证据先拒绝相关字段，不伪造-1。TYPE-BASE。 |
| 域/复合约束→typrelid / typbasetype / typtypmod / typndims / typnotnull / typdefault | 当前0/0/-1/0/false/NULL仅适用这些内建非域行；无域约束或复合身份来源。 | oid26/oid26/int4 23/int4 23/bool16/text25，长度4/4/4/4/1/-1。非复合typrelid=0；非域basetype=0、typtypmod=-1、typndims=0、notnull=false，无类型默认值时default SQL NULL。列typmod不得写到共享类型行；域须保留自己的OID与真实基类型/约束，缺源在CREATE DOMAIN/TYPE副作用前0A000。TYPE-DOMAIN。 |
| owner/ACL及其他客户端字段→typowner / typacl等 | 当前owner NULL且描述20；typacl绑定1034但固定行无真实ACL来源。无真实PG函数/排序规则/存储属性注册证据。 | owner目标oid26、非NULL且关联角色；ACL目标aclitem[]1034、可NULL仅表示真实默认权限。缺owner/ACL语义在字段取值前0A000，不把NULL当已完成权限模型。typinput/output/receive/send等regproc24、typcollation oid26及typalign/storage内部char18若被请求须逐项接真实注册/构建来源；不造函数OID、不统一0。权限及函数窄块后续补证。TYPE-ERROR。 |
| 展示→format_type(oid,int4) | 同一类型身份+列声明typmod，当前format_column_type只覆盖表内子集。 | text/25、长度-1；NULL类型OID→SQL NULL，NULL修饰符与-1区别保留。目标“未知OID”只指参考目录确实不存在的OID：0→“-”、无效非0→“???”；PG有效但本端尚未映射如2950不得冒充未知，查询产品端须0A000门禁，或完成注册后返回uuid。名称引用/path沿第2/5章；不还原整列DDL。TYPE-FORMAT/UNKNOWN。 |

上述字段自身typmod均-1，typtypmod字段值与输出描述typmod不同。最小闭环除第3章逐族标量外，还必须包括已在目录描述使用的name19、内部char18、oid26、regclass2205、数组及vector；这只是目录身份与字段事实要求，不自动开放通用参数、运算及DDL。没有用户类型且查询限定public时，类型正确空集合理；必需内建缺行不能以空集充数。JSON/JSONB、UUID、timestamptz、interval等依第3章逐族门槛，不能因固定OID可查就标记可用。

官方依据（访问日期2026-10-24）：PostgreSQL **18** [pg_type](https://www.postgresql.org/docs/18/catalog-pg-type.html)定义字段、容器/域关联；[系统信息函数](https://www.postgresql.org/docs/18/functions-info.html)定义format_type签名；固定OID/名称/配对取官方**REL_18_0** [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)，NULL/无效OID、bpchar及名称展示取同tag [format_type.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/format_type.c)。客户端必要typelem/typdelim字段由官方**pgjdbc REL42.7.8** [TypeInfoCache.java](https://raw.githubusercontent.com/pgjdbc/pgjdbc/REL42.7.8/pgjdbc/src/main/java/org/postgresql/jdbc/TypeInfoCache.java)::prepareGetArrayDelimiterStatement/prepareGetArrayElementOidStatement提供来源；不推定本轮运行了驱动。18.6仍是来源指定目标，无该补丁参考构建或精确运行取证。

#### 索引目录与定义反解

以下为目标及缺口，非当前支持声明。官方依据（2026-10-24查阅，PostgreSQL **18**）：[pg_index](https://www.postgresql.org/docs/18/catalog-pg-index.html)、[pg_get_indexdef](https://www.postgresql.org/docs/18/functions-info.html#FUNCTIONS-INFO-CATALOG-TABLE)、[CREATE INDEX](https://www.postgresql.org/docs/18/sql-createindex.html)；固定 **REL_18_0** 的 [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)核对目录类型身份，[ruleutils.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/ruleutils.c)::pg_get_indexdef_worker 用于反解边界复核。客户端需求以 [psql 18](https://www.postgresql.org/docs/18/app-psql.html) 的 `\di`、`\d public.idx_contract` 与 `-E` 为官方入口；下方SQL均为人工契约探针，具体psql隐藏SQL另采。来源指定18.6的参考构建、精确错误字段及客户端执行均未取证。

所有字段在同一当前库/快照按真实索引身份关联。目录SELECT权限与对象读权限分开：具有目录SELECT而无表SELECT的同库角色仍可见这些目录行及定义，不以search_path过滤；普通读表仍作权限检查。名称解析/可见性继承前块，第14/15块补权限与可见性函数；不得因权限未取证伪造owner或把应有索引清空。

| 字段/查询→验收 | 真实来源与关联键 | PG类型/OID（typlen） | 目标、当前缺口与不支持策略 |
| --- | --- | --- | --- |
| indexrelid、indrelid→IDX-BASE/HANDLE/LIFE | 表持久ID、IndexInfo.ID或有证据的句柄-1；分别连pg_class.oid（索引/所属表） | oid/26（4）两列 | 两个外键与regclass一致；索引名字不作全局键，遵守关系块schema内关系名唯一性，不能把各表PRIMARY重名直接当PG合法命名。非法ID/碰撞/越界沿关系块拒绝，不能哈希或按枚举顺序编号。 |
| indnatts、indnkeyatts→IDX-BASE/SHAPE | 有序键列及独立包含列元数据；与索引pg_class.relnatts闭环 | int2/21（2）两列 | 总列数/键数分别保真；当前都等于Columns.len只在无INCLUDE时可能成立，缺INCLUDE来源时不降为键列。 |
| indisunique、indisprimary、indnullsnotdistinct→IDX-FLAGS/SHAPE | Unique、Primary、真实NULL唯一性策略；主键必须唯一 | bool/16（1）三列 | 当前前两项取模型、第三项固定false；仅经证明NULL互异语义才可false。NULLS NOT DISTINCT缺源应在DDL副作用前拒绝；既有不等价状态在输出前0A000。 |
| indkey→IDX-ORDER/HANDLE | Columns顺序映射到所属表持久PG attnum，不能仅信当前Offset+1 | int2vector/22（-1） | 长度=indnatts，零基下标；键在包含列前，表达式槽为0。遵循列块DROP/ADD槽位契约；错误Offset/缺列/重复属性槽位输出前拒绝，不静默重排或丢列。 |
| indoption→IDX-OPTIONS | 方法特定逐键排序/NULL顺序 | int2vector/22（-1） | 长度=indnkeyatts；btree默认ASC NULLS LAST才为0，DESC/NULLS FIRST必须保存真实位标记；缺源拒绝相关形态，不一律全0。 |
| indclass、indcollation→IDX-AUX | 每个键的真实opclass/collation身份，分别连pg_opclass/pg_collation | oidvector/30（-1）两列 | 长度均=indnkeyatts；无collation语义的类型其collation可0，有排序规则则须真OID；opclass必须关联实际方法/类型。当前空indclass、全0collation不是完整契约，缺源请求在输出前0A000，不能用NULL/空vector掩盖。 |
| indexprs、indpred→IDX-SHAPE | 表绑定的表达式树/谓词；表达式与indkey中0一一对应，谓词独立保留 | pg_node_tree/194（-1）两列 | 普通整列无谓词时SQL NULL合法；当前描述为text/25是缺口，不能以NULL值免除类型要求。复杂形态未实现则DDL副作用前或既有对象目录输出前0A000，不返回丢表达式的可执行定义。 |
| indisvalid、indisready→IDX-STATE | 真实建索引/回填生命周期及读写可用语义 | bool/16（1）两列 | StatePublic仅为候选依据，不能直接等同PG两状态；当前固定true，须证明同步完成的普通索引后才开放。中间态不能假装有效，也不能仅靠空集宣称支持CONCURRENTLY。 |
| indisexclusion、indimmediate、indisclustered、indcheckxmin、indislive、indisreplident→IDX-EXTRA | 排他/延迟约束、CLUSTER、版本可见性、删除生命周期、复制身份各自真实元数据 | bool/16（1）六列 | 当前未绑定；缺源字段请求在绑定/输出前0A000。禁用特性可建立经证明的固定策略，但不能无来源补false/true；特殊DDL未开放前拒绝，具体子系统交后续独立任务。 |
| pg_get_indexdef(oid)及(oid,int4,bool)→IDX-DEF/QUOTE/UNKNOWN | 同一真实索引定义、方法/类型、schema与已转义标识符 | text/25（-1），参数26或26/23/16 | 列号0为整定义，1起为索引列序的片段；pretty只影响格式，不改变含义。保留唯一性、方法、排序、包含列、表达式和谓词；还原文本是重构结果，不保证原DDL字面相同。未知OID与NULL按参考契约返回NULL，对已存在索引，越界列按REL_18_0源码目标为空字符串（非NULL）；18.6精确运行另采。当前NULL返回是待修差异。 |

所有字段自身typmod目标-1，text/binary及零行描述沿第3章；向量不能写成int2[]/oid[]，pg_node_tree不能写成text。仅当模型、目录辅助身份及方法行为均有等价证据时才输出可重建定义；Tp=Invalid不能无依据推断btree，Invisible/Global或本地前缀索引也不能默认为普通PG索引。若某形态不等价，产品目标0A000必须早于副作用/成功行；这是产品能力门槛，不伪称PostgreSQL对合法索引形态也拒绝。

#### 约束目录与定义反解

以下为目标及缺证门槛，不是已支持清单。官方依据于2026-10-24查阅：PostgreSQL **18** [pg_constraint](https://www.postgresql.org/docs/18/catalog-pg-constraint.html)用于字段与关联、[系统信息函数](https://www.postgresql.org/docs/18/functions-info.html)用于反解签名；固定 **REL_18_0** 的 [pg_constraint.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_constraint.h)核对约束名在单关系内唯一和FK支持索引位于父表，[ruleutils.c::pg_get_constraintdef_worker](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/ruleutils.c)核对返回ADD CONSTRAINT之后的片段及未知OID→NULL（源码推导）。类型OID沿第3章并核对同tag [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)；[CREATE TABLE](https://www.postgresql.org/docs/18/sql-createtable.html)、[ALTER TABLE](https://www.postgresql.org/docs/18/sql-altertable.html)用于约束和验证目标。[psql 18](https://www.postgresql.org/docs/18/app-psql.html)说明用-E取得\d真实查询；下方SQL除仓库模板外均为人工验收探针，未称为psql/DataGrip实采。**18.6精确构建/运行与诊断均未取证**，上述大版本手册及18.0源码不能替代。

公共前提：同一当前库/namespace/角色上下文和单次元数据快照；系统关系pg_constraint的OID2606与用户约束行OID分开。字段名均按真实声明保留引用；约束名不跨表唯一，不能以conname单独作键。目录枚举与定义函数不自动按表SELECT权限删行；前提具目录SELECT、函数EXECUTE，普通表读取仍独立授权，角色/可见性按后续专块验证，search_path不得串库。

| 字段/查询→需求 | 真实来源、关联键、PG目标类型/OID及缺失策略 |
| --- | --- |
| oid、conname、contype→CON-BASE/IDENTITY | 约束类别+持久表ID+持久约束ID；oid/26、name/19、内部char/18。p/u须有真实约束来源，不能将每个CREATE UNIQUE INDEX冒充u；当前索引复用OID是适配事实而非独立PG分配证明。CHECK.ID仍需独立无碰撞映射，不能占用FK/虚拟主键已用区间；名称hash/枚举序号禁止。无持久来源时拒绝该形态开放，不伪造行。 |
| conrelid、connamespace→CON-BASE/JOIN | 当前真实表及所属namespace，均oid/26，关联pg_class.oid及pg_namespace.oid；本块表约束均非0，域不套表OID。模型缺失/身份溢出或关联丢失在输出成功行前拒绝。 |
| conindid、confrelid→CON-JOIN | 均oid/26。p/u支持索引属于conrelid；f的conindid必须是confrelid上的真实引用唯一索引，不能用子表辅助索引；c无支持索引为0。非f的confrelid为0。复用索引块PKIsHandle门槛，不能重复投影。 |
| conkey、confkey→CON-ORDER/CHECK | int2[]/1005（非int2vector/22），1起始数组，下标与键位置有序；由真实列身份关联本表/父表pg_attribute(attrelid,attnum)。FK逐对映射并保留声明顺序，不排序列名或ID；CHECK由表达式依赖解析与ConstraintCols核对，无列依赖的常量CHECK目标conkey为SQL NULL，依据REL_18_0 [pg_constraint.c::CreateConstraintEntry](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/catalog/pg_constraint.c)的零键分支（源码推导），不能拿NULL或空数组掩盖未知依赖。非FK的confkey为SQL NULL。Offset重排缺口沿列契约；缺列、隐藏/非公开列、长短不等或坏Offset拒绝。 |
| confupdtype、confdeltype、confmatchtype→CON-ACTION | 内部char/18；f的动作a/r/c/n/d分别对应NO ACTION/RESTRICT/CASCADE/SET NULL/SET DEFAULT，match为s/f/p语义，非f动作及match为无适用码的内部char零字节，不能输出一个空格冒充PG零字节。当前原生OnUpdate/OnDelete整数映射0或4→a、1→r、2→c、3→n、5→d仅是静态实现，不证明执行效果等价；无match字段不得猜MATCH FULL或其NULL语义。开放简单默认match前也须来源与行为门槛。 |
| condeferrable、condeferred、convalidated、conenforced→CON-STATE | 均bool/16。延迟及初始模式须持久事实，false仅适用于已明确限制NOT DEFERRABLE且实际即时执行的对象；NOT VALID和VALIDATE须有历史行扫描及后续写入检查证据。无原生延迟/验证标记，不能从StatePublic推true或全填false。CHECK.Enforced可作为执行开关候选，须与真实执行一致；p/u/f强制执行也须证明。无法保真时对相关建约束请求在副作用前0A000，既有原生不可判定状态在字段输出前0A000。 |
| conbin、pg_get_constraintdef(oid[,boolean])→CON-CHECK/DEF/UNKNOWN | conbin为pg_node_tree/194，CHECK的真实绑定表达式经PG语义转换，非CHECK为SQL NULL；不能把ExprString原文贴成PG节点树。反解返回text/25、typmod−1，保留列顺序、引用对象、动作/匹配、延迟/验证/执行/继承等实际适用选项；返回约束片段而非完整ALTER TABLE，pretty只改格式。缺对象与strict NULL返回SQL NULL；有对象但无法保真必须0A000，不把CHECK或非等价对象隐藏成不存在。conbin无法提供PG树时明确拒绝此字段，不妨碍以后另行证明基于规范表达式的定义反解。 |
| connoinherit、conislocal、coninhcount、conparentid、contypid→CON-EXTRA | bool16/bool16/int2-21/oid26/oid26；分别来自真实继承策略、本地来源、祖先计数、父分区约束身份、域类型身份。普通无继承表约束须按类别判定：普通CHECK默认noinherit=false，普通非分区FK为true（REL_18_0 [tablecmds.c::addFkConstraint](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/commands/tablecmds.c)源码推导）；其余四项在分类证明后才可true/0/0/0；当前仅noinherit固定false，其余未绑定。不支持对象必须拒绝，不能用该普通组常量覆盖域/继承/分区。 |
| conexclop、conpfeqop、conppeqop、conffeqop、confdelsetcols、conperiod→CON-EXTRA | 前四项oid[]/1028关联真实pg_operator，分别为排除及FK三种比较方向；第五int2[]/1005为DELETE SET动作列子集，第六bool/16为时态约束标记。无适用对象可返回NULL数组、普通非时态false须有分类证据；FK比较操作符缺身份不能返回空数组充作成功。缺排除/时态/子集动作来源时在DDL前拒绝，既有对象相关输出前拒绝。 |
| xmin、仓库18列模板→CON-TEMPLATE | xmin标准为xid/28，当前NULL int8不是该身份；禁止用TSO或计数伪造。无真实PG事务标识时，仅在模板允许可空状态提示且显式记录偏差时返回有类型NULL，完整PG xmin声明仍不开放。模板有两个namespace参数，需同时绑定本端public OID；NULL conexclop经array子查询可得{}，与原字段NULL不同。conbin表达式不适用时允许NULL，但CHECK必需定义不能以NULL掩盖。 |

固定宽度目标：oid/26与xid/28为4、name/19标准构建64、内部char/18及bool/16为1、int2/21为2，数组/pg_node_tree/text为−1，所有上述目录表达式typmod−1；内部零字节在DataRow是长度1的载荷，不与SQL NULL长度−1混同。所有字段text/binary Describe及载荷沿第3章验证，name/text、194/25及28/20差异须如实登记。真实目录无约束才允许类型正确空集；不支持字段在分析/绑定期拒绝，坏元数据或表达式无法保真在执行输出前拒绝；产品0A000不冒充PG对合法请求的拒绝。

#### 默认表达式目录契约

目标依据（PostgreSQL 18，访问 2026-10-24）：[pg_attrdef](https://www.postgresql.org/docs/18/catalog-pg-attrdef.html) 定义四字段及列关联；[pg_get_expr](https://www.postgresql.org/docs/18/functions-info.html#FUNCTIONS-INFO-CATALOG-TABLE) 定义二/三参数反解和无 Vars 时 relation=0；[默认值](https://www.postgresql.org/docs/18/ddl-default.html) 定义插入时求值；[生成列](https://www.postgresql.org/docs/18/ddl-generated-columns.html) 定义表达式限制和 STORED/VIRTUAL；[columns](https://www.postgresql.org/docs/18/infoschema-columns.html) 提供默认/生成字段及权限区别。仅作为大版本目标；18.6 参考构建、精确错误位置及运行结果未取证。

| 需求→字段/调用 | 真实来源、关联及 PG 类型（typmod 均 −1） | 目标与不支持策略→验收 |
| --- | --- | --- |
| DEF-ID→oid | 独立默认对象持久身份，关联表 ID/稳定列 ID/对象生命周期；oid/26，4字节非NULL | 当前无来源，待建立且不可复用表OID或临时位置；不得 hash/填0/NULL。删除默认后对象消失，SET 新默认的对象身份变化须按参考构建取证→DEF-META。 |
| DEF-KEY→adrelid/adnum | 当前表OID→pg_class.oid，Offset+1→同表pg_attribute.attnum；oid/26（4字节）、int2/21（2字节）非NULL | 必须与公开列同源，不能把列ID直接当属性号；真实有默认或生成表达式才有行，缺对象为空集→DEF-BASE/META。 |
| DEF-TREE→adbin | 持久、已绑定的默认/生成定义及列/类型/函数依赖；pg_node_tree/194，变长非NULL | 是内部表达式表示，不能冒充普通SQL；当前私有relid前缀不是PG树。需要真实转换与codec，否则读取必需定义在成功行前0A000；不以NULL/空集掩盖→DEF-BASE/GENERATED。 |
| DEF-DEPARSE→pg_get_expr(adbin,relation[,pretty]) | 同一表达式定义/依赖与关系上下文；参数194/26/[16]，返回text/25，变长可NULL | 非NULL树重建语义等价SQL，默认pretty=false；pretty仅格式变化；无Vars允许0，有列引用须正确上下文。NULL输入正常传播；非法树/缺上下文须拒绝，不回显任意文本。精确PG错误另采；产品暂不支持树转换时执行前0A000→DEF-BASE/ARG。 |
| DEF-PROJECTION→atthasdef/attgenerated、column_default/generation_expression | 同一列定义；前两项bool/16、内部char/18；后两项information_schema.character_data域（底层varchar/1043，域OID须由参考构建取证） | 无默认、SQL NULL默认、文本'NULL'、常量、运行时函数、生成表达式分开。information_schema仅显示当前用户有权访问的列，生成列的column_default为NULL、generation_expression有定义；pg_catalog不套用该视图权限过滤。复用列块与后续information_schema块，不在此宣称视图已接线→DEF-BASE/GENERATED/ACCESS。 |

安全翻译以已绑定类型与语义为前提：整数7、bool true、text的O'Reilly经正确引用/转义后可作为最小子集；CURRENT_TIMESTAMP须核实第3/4/5章时区、事务时间和精度，不能仅改拼写。原生 IFNULL、反引号标识符、未解析函数文本、无真实序列引用的nextval、未知编码/类型、零日期须拒绝或进入独立转换任务。显式 DEFAULT NULL 是否形成目录行须采参考构建的归一行为，不要求凭空制造对象；但客户端不能把无行/SQL NULL与文本常量'NULL'混淆，不能靠固定NULL替代必需定义。

官方客户端证据：[psql REL_18_STABLE describe.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/bin/psql/describe.c)::describeOneTableDetails 在列详情子查询按 adrelid/adnum/atthasdef 调三参数 pg_get_expr（pretty=true）。这证明查询形状需求，分支可变且未固定客户端构建；本块下方是人工探针，并非完整psql抓包。DataGrip/JDBC/ORM精确版本与SQL未采集。目录可读不授予表SELECT、函数EXECUTE或序列USAGE；内省不得执行默认函数或推进序列。search_path只影响合法名称反解，不能改变存储身份或跨库泄露。

#### 视图定义内省契约

目标依据（PostgreSQL 18，访问 2026-10-24）：[函数手册](https://www.postgresql.org/docs/18/functions-info.html) 定义 OID、pretty、wrap_column 与旧 text 名称签名及返回 SELECT；[CREATE VIEW](https://www.postgresql.org/docs/18/sql-createview.html) 定义创建、替换及 CHECK OPTION；[pg_views](https://www.postgresql.org/docs/18/view-pg-views.html) 定义名称、owner、重建定义字段。官方 [REL_18_STABLE ruleutils.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/utils/adt/ruleutils.c)::pg_get_viewdef_worker 无 SELECT 规则时返回 NULL，支持区分正常不存在与损坏定义；[psql describe.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/bin/psql/describe.c)::describeOneTableDetails 在 verbose 视图/物化视图详情调 OID、true。分支可变，均非18.6构建证据；该补丁版本的精确行为、错误位置与运行结果未取证。

| 需求→字段/调用 | 真实来源、关联键、PG类型（typmod −1） | 目标、不支持策略→验收 |
| --- | --- | --- |
| VIEW-ID→pg_class.oid/relnamespace/relname/relkind | 共享公开 TableInfo.ID→table_oid，所属schema→namespace_oid；分别oid/26、oid/26、name/19、内部char/18；非NULL | 沿关系/namespace块，与pg_attribute.attrelid同源；普通View为v，物化视图m须真实分类。名称不能hash造身份，重命名不改身份→VIEW-BASE/META。 |
| VIEW-DEF→pg_get_viewdef(oid[,pretty]) | 同一持久视图定义、输出列Cols及已绑定对象/类型依赖；参数26/[16]，返回变长text/25可NULL | 返回语义等价的PG SELECT，省略CREATE头与CHECK OPTION；pretty=false默认，true只改可读格式。当前切片不足以证明可重建；缺可靠转换的已存在视图在成功行前0A000，不返回原生SQL、空串或假NULL→VIEW-BASE/LEX/REJECT。 |
| VIEW-ARG→非视图、未知OID、NULL与其他签名 | 当前库稳定OID查找及函数重载元数据；输出text/25 | 普通表OID、无对象OID分别正常NULL；NULL输入传播。未知OID不能泄露其他库。OID+int4/23 wrap签名目标启用pretty；旧text/25名称按第2章绑定，缺支持在分析/绑定前0A000且不误选bool签名；错误参数拒绝而非隐式当true→VIEW-ARG。 |
| VIEW-META→CHECK OPTION与schema限定 | ViewInfo.CheckOption、Security及真实依赖/Cols；定义仍text/25 | CHECK OPTION不属于返回SELECT，单独保存，不能从注释/字面量误删；模式限定按第2/5章及当前path安全输出，双引号保大小写，原生库名不冒充PG schema。重命名依赖后仍绑定同对象，无绑定来源不能保证时显式拒绝→VIEW-LEX/PATH/META。 |
| VIEW-PROJECTION→pg_views.schemaname/viewname/viewowner/definition | 同一schema/关系/真实owner及定义；前三name/19，definition text/25 | 仅普通视图；当前provider未有本轮接线证据，owner映射缺口不能造字符串。该视图及information_schema.views留对应窄块实现，定义必须与函数一致；information_schema权限过滤不得直接套pg_catalog→VIEW-ACCESS。 |

定义是重建结果，不承诺原输入注释、空白、关键字大小写逐字回放；字符串内容和引号标识符语义必须保留。原生SELECT字符串只有经绑定、类型和方言验证后才可反解，不能对文本全局替换引号或按“ AS ”切割。内省不执行视图查询、不读取基表数据、不改变CHECK OPTION；其写入约束运行语义交后续实施，不能由定义出现推出已可用。权限须核对真实角色及目录读取权：pg_get_viewdef可读不授予基表SELECT，不能一律要求视图owner，也不能绕过已撤销的目录访问权限。

#### 序列身份与依赖契约

本块字段逐项沿首块模板登记，以下类型均为未强转的目标类型。OID 数据值与其 Describe 类型 OID 分开；所有行来自同一次快照、当前 PG 数据库，权限与 search_path 复用本章公共契约。目录读取不等于拥有序列 USAGE/SELECT/UPDATE；目录参数和依赖不应因无序列分配权限而被伪造为空，名称可见性也不等于目录可枚举性。跨库对象不泄漏，所有者角色/ACL 尚无来源，不从“列拥有”关系推造 relowner。

| 字段/查询 | 真实来源与关联键；目标 PG 类型/OID | 目标与缺口策略；验收 |
| --- | --- | --- |
| pg_sequence.seqrelid、seqtypid | TableInfo.ID→pg_class.oid，实际声明类型→pg_type.oid；均 oid/26 | 只为真实 relkind=S 对象一行；类型 smallint/integer/bigint 对应 21/23/20，当前固定20只代表原生序列。缺声明类型不得猜测；SEQDEP-01。 |
| seqstart、seqincrement、seqmin、seqmax、seqcache | SequenceInfo.Start/Increment/MinValue/MaxValue/Cache 与 CacheValue；均 int8/20 | 五项非NULL参数，禁用缓存有效大小1，不返回水位或 currval；SEQDEP-01/04。 |
| seqcycle | SequenceInfo.Cycle；bool/16 | 非NULL，文本/二进制编码遵第3章，零行也须同描述；SEQDEP-01。 |
| pg_depend.classid、objid、refclassid、refobjid | 真实依赖两端目录类别与持久对象 ID；均 oid/26 | 序列→schema 为 (1259,S,2615,N)；序列→拥有表列为 (1259,S,1259,T)，不是 pg_sequence 的关系OID2224。缺拥有来源时当前列拥有子查询可类型正确空集，但不得据此开放 identity；SEQDEP-02/03。 |
| objsubid、refobjsubid、deptype | 真实依赖端点列号与种类；int4/23、int4/23、内部单字节 "char"/18 | 序列端0；schema端0/n；OWNED BY 表列端为稳定 attnum/a，identity 为 attnum/i。列号须跟 pg_attribute 一致，不能永久依赖可重排的数组位置；SEQDEP-02/05。 |
| pg_attribute.attidentity；pg_get_serial_sequence(text,text) | 持久 identity 模式与同一拥有边；"char"/18；text/25 | 普通列空、ALWAYS=a、BY DEFAULT=d；函数按真实关联返回可解析限定名称，合法普通列无关联才NULL，不扫描 nextval 文本或拼默认名称。当前无来源/函数绑定时在绑定或能力门禁以0A000拒绝未开放能力，不能无条件NULL伪装成功；SEQDEP-03。 |

显式独立 sequence、普通列的 nextval 默认值引用、serial/OWNED BY 和 identity 分开：默认引用本身不产生 a/i 拥有边；pg_attrdef 的正常引用依赖需来自已解析对象引用，沿默认表达式块，不在此虚构。identity 的 i 与 serial 的 a 依赖沿第5章；identity 不强造 pg_attrdef/nextval 默认文本。DDL 开放前必须具备模式、稳定关联及同一序列参数事实，AUTO_INCREMENT 继续保留原生身份。

官方目标均于 **2026-10-24** 查阅，适用 **PostgreSQL 18**：[pg_sequence](https://www.postgresql.org/docs/18/catalog-pg-sequence.html) 对应参数字段；[pg_depend](https://www.postgresql.org/docs/18/catalog-pg-depend.html) 对应端点类型和依赖生命周期；[identity](https://www.postgresql.org/docs/18/ddl-identity-columns.html)、[CREATE SEQUENCE](https://www.postgresql.org/docs/18/sql-createsequence.html) 对应真实对象及 OWNED BY；[系统信息函数](https://www.postgresql.org/docs/18/functions-info.html) 对应 pg_get_serial_sequence 的关联及名称规则。官方 [REL_18_STABLE sequence.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/commands/sequence.c)::process_owned_by 的 for_identity 分流支持 i/a 推导，分支可变，不是18.6固定证据。[psql 18](https://www.postgresql.org/docs/18/app-psql.html) 定义 `\ds`、`\d` 与 `-E` 采集入口；下列 SQL 为人工契约探针，不声称 psql/JDBC/DataGrip/ORM 自动生成。精确18.6参考构建、客户端版本、生成SQL及运行行为均未取证。

#### 函数目录与注册边界

字段与查询沿首块证据模板、类型映射沿第3章；目录SELECT权限、schema USAGE、函数EXECUTE及名称可见性分别判断。以下为目标类型，零行也保留Describe；当前数据库及同一元数据代际内关联，不按search_path删除目录行。

| 字段/查询 | 真实来源/关联键；目标 PG 类型及类型OID | 目标、不支持策略与验收 |
| --- | --- | --- |
| oid、proname、pronamespace | 方言签名对象身份、名称、所属namespace→pg_namespace.oid；oid/26、name/19、oid/26 | 内建身份受控稳定分配，用户对象须持久ID；签名键含namespace和输入类型序列，不能按名称合并重载。当前proname映射text/25及合成OID只是适配器现状，不冒充PG原值；PROC-01/03。 |
| proargtypes、prorettype、provariadic | 同一签名输入参数/返回/可变参数元素→pg_type.oid；oidvector/30、oid/26、oid/26 | IN/INOUT/VARIADIC输入有序，OUT不计入身份；非可变为0。当前三个原语均26→25，不能从MySQL EvalType猜int4/int8重载；类型身份缺失时拒绝开放签名；PROC-01/03。 |
| proargnames、proargmodes、proallargtypes、proargdefaults | 真实参数声明、模式、全部参数及已解析默认表达式；text[]/1009、内部char[]/1002、oid[]/1028、pg_node_tree/194 | 无名称/全IN/无OUT等情形按PG字段规则NULL；无默认NULL，不能复制SQL字符串冒充内部表达式。当前proargmodes使用TextArray、proargdefaults为text，且pronargs/int2/21、pronargdefaults/int2/21未绑定，均需补元数据后验收，未实现字段绑定期0A000；PROC-01。 |
| prokind、proretset、provolatile、proisstrict、prosecdef、proleakproof、proparallel | 第4章实际执行类别、返回形态与实现语义；内部char/18、bool/16、内部char/18、bool/16、bool/16、bool/16、内部char/18 | 当前f/false/s/false/false/false/u仅为三个原语的声明；聚合需关联pg_aggregate，窗口/集合返回另沿第4章。不因展示行而允许折叠、并行或提权；PROC-02/03。 |
| prolang、prosrc、proconfig | 实际语言接口→pg_language.oid、真实实现标识、函数配置；oid/26、text/25、text[]/1009 | 当前12/internal、实现名、NULL配置为适配器事实；内部原语无SQL body可返回NULL，不能为不存在用户例程伪造源码；PROC-02。 |
| proowner、proacl | 真实角色身份→角色目录、同一函数ACL；oid/26、aclitem[]/1034 | 当前owner是NULL且绑定bigint/20，ACL只是补NULL，无角色或授权依据。PG中NULL ACL表示默认权限，不能解释为拒绝全部或授权全部；开放权限函数前需真实来源，无法证明授权时能力门禁拒绝，不编造owner/ACL；PROC-04。 |
| procost、prorows、xmin | 真实代价、基数及MVCC版本；float4/700、float4/700、xid/28 | 当前分别NULL/0/NULL且均绑定bigint/20，不能宣称PG类型兼容；无代价或事务来源时记录缺口并拒绝依赖该语义的探针，不生成虚假事务号；PROC-01。 |
| pg_get_function_arguments(oid)、pg_get_function_result(oid)、pg_get_function_sqlbody(oid) | 同一函数对象的完整声明/返回声明/SQL标准body；text/25 | 绑定、格式化与目录签名同源；当前只认三个适配器OID，未知OID走0A000是当前边界，不声称PG原生错误码。NULL与internal无SQL body分开验收；PROC-02。 |

目录provider负责事实投影，目录有界求值器负责已绑定白名单表达式；普通SQL解析/重载/隐式转换、工厂与执行核归第4章，wire描述归第3/7章。每个宣称可调用的系统函数都须有普通SQL链证据；不能只向FUNCTION_INTROSPECTION加名字，也不能把全部MySQL funcs导出pg_proc。用户函数DDL尚未开放时，只允许对确实不存在的用户例程类型正确空集；真实已注册必需函数缺失不能靠全空集通过。

官方出处于 **2026-10-24** 查阅，版本为 **PostgreSQL 18**：[pg_proc](https://www.postgresql.org/docs/18/catalog-pg-proc.html) 规定签名及字段语义；[系统信息函数](https://www.postgresql.org/docs/18/functions-info.html) 规定函数声明反解、权限与可见性；客户端官方 [psql 18](https://www.postgresql.org/docs/18/app-psql.html) 提供 `\df` 及 `-E` 隐藏SQL采集入口。下列SQL是人工契约探针，不称客户端生成；精确18.6构建、驱动版本和真实客户端运行均未取证。

#### 对象注释与描述契约

沿首块逐字段证据模板；当前证据仅为上节静态源码与测试意图。同一数值OID跨目录不代表同一对象，JOIN必须同时限制对象类别。限定目录查询不按search_path删除行，名称解析沿第2/5章。

| 需求→字段/查询 | 真实来源与关联 | PG目标类型/OID、缺失策略→验收 |
| --- | --- | --- |
| pg_description.objoid | table_oid(model.ID)，索引为index_oid(表ID,索引ID)；列仍是所属表OID | oid/26、typlen4，与本库pg_class.oid一致；不用列ID作objoid。缺身份输出前0A000。COM-ROWS/KEY。 |
| classoid | 当前关系注释为1259，即pg_class系统关系OID，不是namespace | oid/26、4，关联pg_class系统目录行。函数/类型/namespace描述分别需要pg_proc/pg_type/pg_namespace身份与真实注释来源；当前缺来源不造行，请求写入则副作用前0A000。COM-KEY。 |
| objsubid | 表/索引/序列为0，列关联pg_attribute.(attrelid,attnum)；当前Offset+1 | int4/23、4；目标沿列目录稳定槽位，不可重排后配错注释。无持久槽位来源时相关DDL前或既有不明状态读取时0A000。共享描述无此列。COM-LIFE。 |
| description | 完整模型Comment，序列/索引用其真实字段 | text/25、-1；真实行非NULL，保留Unicode、引号、换行及反斜杠。无注释为零行，LEFT JOIN才扩为SQL NULL。PG COMMENT空串与NULL均删除，不能制造空字符串描述行；原生空String映射无行，不声称能恢复其历史来源。COM-TEXT/EMPTY。 |
| pg_shdescription.objoid/classoid/description | 数据库身份沿数据库目录、classoid=1262；当前DBInfo无Comment持久来源 | oid/26、oid/26、text/25；无注释数据库合法零行，LEFT JOIN保行NULL。数据库COMMENT缺来源时副作用前0A000，不能假成功再返空。角色/表空间各缺真实身份与注释来源，无对象时不造行，各自未支持写入0A000。COM-SHARED。 |
| obj_description(oid,name)、col_description(oid,int4) | 前者按objoid/catalog对应classoid且objsubid=0；后者classoid=1259且匹配列号，共享同一快照 | 参数OID26/19、26/23→text/25；无描述或NULL输入返回SQL NULL，不能用obj_description取列注释。当前未注册目录函数在绑定阶段0A000，不把缺实现当无注释。弃用单参数obj_description(oid)未支持时绑定0A000，若开放须保留跨目录歧义限制。COM-FUNC。 |
| shobj_description(oid,name) | pg_shdescription按objoid及catalog对应classoid定位，不读本库pg_description | 参数26/19→text/25；无注释为NULL。当前未接线绑定0A000；无共享持久来源不承诺非空数据库描述。COM-SHARED/FUNC。 |

以上输出typmod=-1，oid/int4宽4，text变长；来源关系/属性号、text/binary与零行Describe沿第3/7章。注释读取与对象SELECT/schema USAGE分开：PG18允许已连接用户读取本库对象注释，共享注释在任何数据库可读；无表SELECT不应隐藏注释。名称解析可能先因USAGE不足失败，数字OID探针不据此删描述。COMMENT写入要求owner或适用管理权限，权限不足目标42501且无变化；当前owner/ACL来源和运行证据不足，不能虚报已实现。

官方目标（PostgreSQL **18**，访问日期 **2026-10-24**）：[pg_description](https://www.postgresql.org/docs/18/catalog-pg-description.html)、[pg_shdescription](https://www.postgresql.org/docs/18/catalog-pg-shdescription.html)定义字段与共享范围；[注释函数9.27.6](https://www.postgresql.org/docs/18/functions-info.html#FUNCTIONS-INFO-COMMENT)定义签名和无注释NULL；[COMMENT](https://www.postgresql.org/docs/18/sql-comment.html)定义空串删除与读写权限；[psql 18](https://www.postgresql.org/docs/18/app-psql.html)提供描述命令与-E查询采集入口。类型OID沿第3章已核来源。以下SQL均为人工契约探针，未采集特定psql/JDBC/DataGrip/ORM版本的真实生成SQL；18.6精确构建及运行行为未取证。

#### 角色权限与所有权契约

认证身份（原生user/host）、session_user（会话授权角色）、current_user/current_role（有效权限角色）、对象owner和grantor/grantee必须分别建模。登录成功只证明认证路径成功；既不赋予全部对象权限，也不把连接用户补成历史对象owner。PG角色是集群共享身份，须有持久稳定OID与原生身份映射；同名不同Host、名称含@或引号的映射冲突必须显式拒绝，不能继续字符串截断。

| 需求→字段/查询 | 真实来源/关联与当前证据 | PG目标类型、可见性及缺失策略→验收 |
| --- | --- | --- |
| 用户身份→pg_user.usename/usesysid | 当前mysql.user.User，不含Host/稳定角色ID；usesysid未接线 | name/19、oid/26，非NULL；角色键关联pg_roles.oid（底层pg_authid.oid），pg_user只含可登录角色。公开脱敏目录不要求调用者有mysql.user读取权；没有可信映射时输出前0A000，禁止伪造ID/漏掉真实角色。ROLE-ID |
| 用户属性→usesuper/usecreatedb/userepl/usebypassrls | 当前仅Super_priv，其他无PG策略来源 | 均bool/16、非NULL；分别来自明确超级用户/建库/复制/绕过RLS策略。无功能只有明确产品策略确认为禁用才可false；SUPER不能直接提升为PG超级用户，无可信策略则该投影0A000。ROLE-ID/SAFE |
| 用户保密与默认值→passwd/valuntil/useconfig | 当前未提供 | text/25固定脱敏星号，timestamptz/1184可NULL（确无到期），text[]/1009可NULL（确无默认设置）；不得从凭证哈希造展示数据。缺来源或codec拒绝该投影，不以NULL声称无到期。ROLE-SAFE |
| 角色列表→pg_roles.oid/rolname/rolcanlogin | 当前无provider | oid/26、name/19、bool/16，非NULL；含NOLOGIN角色，与pg_user身份一致；角色存在性为公开目录语义，不能照搬对象权限过滤。缺角色层0A000而非成功空集。ROLE-ID |
| psql角色属性→rolsuper/rolinherit/rolcreaterole/rolcreatedb/rolreplication/rolbypassrls；rolconnlimit/rolvaliduntil | 当前均无PG接线；不得由登录结果推断 | 六个bool/16，int4/23（-1无上限），timestamptz/1184（确无到期可NULL）；其余非NULL。每项来自真实策略、继承及资源限制；缺来源逐字段门禁0A000。ROLE-PSQL |
| pg_roles.rolpassword/rolconfig | 当前无接线 | text/25脱敏星号、text[]/1009可NULL，公开安全投影；pg_authid和pg_shadow原始认证数据不是公开替代视图。未授权直接读取敏感目录目标42501，产品尚未开放目录时0A000且不泄露内容。ROLE-SAFE |
| 成员关系→pg_auth_members.oid/roleid/member/grantor/admin_option/inherit_option/set_option | 原生Manager::ActiveRoles是候选，不证明PG成员边 | 前四列oid/26，后三列bool/16，非NULL；三个角色键关联pg_authid.oid，边OID独立；取真实成员授予事实，不用当前活动角色倒推全图。缺继承/SET/ADMIN边语义时0A000。ROLE-MEMBER |
| session_user/current_user/current_role；pg_get_userbyid(oid) | 当前仅截断身份/固定NULL，正式注册沿第4章 | 均name/19；前三个非NULL，有效角色切换不改变session_user。owner查询由稳定角色OID反查，不能返回调用者名字；NULL参数返回NULL，未知OID的参考端fallback文字需参考构建采证，禁止当前固定NULL充当正确结果。ROLE-OWNER/NULL |
| datdba/nspowner/relowner/typowner/proowner及相关owner显示 | 现有块记录无真实持久owner，部分描述为int8；不从mysql.user列表推断 | 原字段oid/26非NULL，反查name/19；按对象类别及原生ID关联真实owner，跨连接一致、ALTER OWNER后更新。缺来源只拒绝所需投影0A000，不造0、root或NULL。ROLE-OWNER |
| datacl/nspacl/relacl/attacl/typacl/proacl；spcacl | 相关目录已有AclArray标记不证明存在真实授权事实；tablespace当前空关系 | aclitem[]/1034，元素aclitem/1033，来源包括grantor、grantee、权限及grant option；PUBLIC不是普通登录角色。NULL代表默认ACL，空数组代表显式无条目，二者不得互换；没有ACL模型时拒绝投影0A000，不能NULL掩盖。无原生tablespace沿对象块空集策略。ROLE-ACL |
| has_table/schema/database/sequence/function/type/column/any_column_privilege；pg_has_role | 原生Manager请求验证可复用，但PG权限/对象/身份解析未接线 | bool/16、可因NULL参数或不存在OID为NULL；以下签名矩阵逐项取真实有效授权，不能按usesuper/认证成功返回true。缺能力0A000，不返回false冒充“无权限”。ROLE-PRIV/NULL |

所有字段typmod=-1，name标准构建typlen64、oid/int4为4、bool为1、timestamptz为8、数组/text为-1；编码、来源属性描述与零行Describe沿第3章，不把text冒充name或aclitem[]。目录可读、对象USAGE/SELECT/CONNECT、search_path可见性是不同判断；本块不重复下一可见性块。安全目录内部取数应通过受控脱敏provider，禁止让公开pg_user查询直接暴露mysql.user原始列或绕过共享授权。

客户端必需候选签名登记（均为待采集/待实施，不能声称某客户端已调用）：对上述八个has_*函数，用户参数可省略或为name/19、oid/26；对象为text/25或oid/26，末参数权限text/25。has_column另含列text/25或int2/21；has_any_column无列参数；pg_has_role的目标角色为text/25或oid/26。结果均bool/16。函数对象文本按完整重载签名解析；类型对象按类型身份解析。权限按对象种类校验，支持逗号任一匹配及grant option；pg_has_role区分MEMBER、USAGE、SET及ADMIN选项，不能只查活动列表。扩展参数OID/绑定与普通SQL函数注册沿第3/4章，目录白名单不能代替全局实现。客户端若需求has_language/tablespace/server/foreign_data_wrapper/largeobject/parameter_privilege，先登记具体SQL和对象来源；未提供对象模型的签名明确0A000，不假定全部允许。

官方目标出处（PostgreSQL **18**，访问日期 **2026-10-24**）：[pg_user](https://www.postgresql.org/docs/18/view-pg-user.html)、[pg_roles](https://www.postgresql.org/docs/18/view-pg-roles.html)、[pg_auth_members](https://www.postgresql.org/docs/18/catalog-pg-auth-members.html)支持字段、脱敏和共享身份；[权限模型](https://www.postgresql.org/docs/18/ddl-priv.html)支持owner及ACL区别；[系统信息函数](https://www.postgresql.org/docs/18/functions-info.html)支持会话身份及查询签名。官方 [REL_18_STABLE acl.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/utils/adt/acl.c)作为权限函数重载、NULL/不存在OID及错误分支后续逐例核对来源。客户端官方 [psql 18](https://www.postgresql.org/docs/18/app-psql.html)及 [describe.c::describeRoles](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/bin/psql/describe.c)给出角色列表字段；该可变分支不是精确客户端构建。JDBC、DataGrip、ORM的精确版本/官方生成SQL尚未采集，由批次23–25补证；本块人工探针不能冒称客户端SQL。精确18.6构建及运行行为仍未取证。

#### 可见性与名称遮蔽契约

引用第2章“引号与大小写规则”“模式搜索路径解析契约”和第5章“模式与搜索路径契约”；有效路径由当前连接、当前库真实namespace、当前角色USAGE及隐式目录/临时namespace决定，不能用SHOW原字符串替代。省略pg_catalog时它优先于显式用户schema；显式列出时服从该位置。临时关系搜索顺序沿第2/5章，临时schema不参与函数/操作符搜索。路径不可见不等于目录行删除，也不等于对象无SELECT/EXECUTE权限。

| 需求/字段与查询 | 来源、关联键、PG类型/OID | 目标及缺口策略 |
| --- | --- | --- |
| pg_table_is_visible(oid)；关系列表 | 当前库真实关系ID→pg_class.oid，relnamespace→pg_namespace.oid，relname保留PG名称身份；输入oid/26，输出bool/16、长度1、typmod−1 | 同名关系按有效路径只选首个，覆盖表/视图/索引/序列等关系；与未限定regclass及普通绑定一致。当前缺函数实现，绑定能力门禁0A000，不能常量true/空集代替。权限独立沿角色块。 |
| pg_type_is_visible(oid)、pg_function_is_visible(oid) | pg_type.oid/typnamespace/typname；pg_proc.oid/pronamespace/proname/proargtypes，类型OID关联pg_type；输入26→输出16 | 类型按同名遮蔽；函数按名称与输入参数类型签名遮蔽，重载不按名字全部隐藏。真实来源与普通注册沿类型/函数块；缺注册或来源时明确0A000，不伪造用户类型/函数。 |
| regclass输入/显示与关系查询 | 同一快照及OID映射；regclass/2205、oid/26均4字节，转text/25长度−1；名称列name/19沿关系块 | 显式schema不受路径成员限制但仍检查schema USAGE；隐式名选择可见关系，显示必须可重新解析回同OID，必要时限定/引用。NULL传播，未知OID/无效名称的诊断先参考取证，不能把不存在与不支持混淆。 |
| 显式目录与权限 | pg_catalog.pg_class/pg_namespace真实提供器，关联键与字段类型沿已有块；权限源沿角色块 | public前置只影响未限定同名关系；无表SELECT仍可能名称可见，但普通读取必须权限拒绝；无schema USAGE从有效路径排除。Describe无副作用，Execute按第5章时点重读。 |

官方目标依据（PostgreSQL **18**，2026-10-24访问）：[§9.27.3](https://www.postgresql.org/docs/18/functions-info.html#FUNCTIONS-INFO-VISIBILITY)提供可见性签名及对象类别；[schema规则](https://www.postgresql.org/docs/18/ddl-schemas.html#DDL-SCHEMAS-PATH)及[OID别名](https://www.postgresql.org/docs/18/datatype-oid.html)提供路径与regclass依据。客户端官方[psql REL_18_STABLE describe.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/bin/psql/describe.c)的describeTableDetails、类型/函数列表分别使用上述三类可见性条件；分支可变，此处不是固定18.6构建或完整客户端SQL采集。[PG18 namespace.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/catalog/namespace.c)的SQL包装器对不存在OID返回NULL；其可见性检查可使用较新的目录状态，不能机械要求与MVCC扫描行始终同一快照，DDL并发须保留该差异。下列SQL为人工探针，不冒称DataGrip/JDBC/ORM原始查询；18.6精确行为仍未取证。

#### 标准模式表视图映射

范围限定三个标准视图，引用数据库、namespace、关系、视图定义、角色权限和可见性公共契约。对象内部键为(当前database身份,namespace身份,关系持久ID/OID)，对外用(catalog,schema,name)连接；三个标准视图不凭空增加oid列。名称改动保持内部身份；不能只用table_name跨schema连接。

| 需求→字段 | 来源与关联 | PG目标类型、权限及缺失策略→验收 |
| --- | --- | --- |
| IS-SCHEMA→schemata.catalog_name/schema_name/schema_owner | 当前共享连接database；同一pg_namespace名称及真实nspowner→角色名，不用原生库名充schema、不用登录名充owner | 均sql_identifier域（底层name/19）；只列本库owner角色可用或有CREATE/USAGE的schema，空schema有授权仍列。缺owner/ACL来源输出前0A000，不以NULL或全量枚举替代。IS-BASE/ACL/EMPTY。 |
| IS-SCHEMA-NULL→default_character_set_catalog/schema/name、sql_path | PG不提供的标准特性，非原生charset/collation字段 | 前三sql_identifier，sql_path为character_data；四列为有类型SQL NULL，此处NULL有官方依据，不是缺事实占位。IS-COLS。 |
| IS-REL→tables.table_catalog/table_schema/table_name；views同三列 | 当前database、共享namespace、同一pg_class关系模型；同库schema+name对应唯一关系 | sql_identifier域，值非NULL；tables与views按owner可用角色、表权限或任一列权限过滤，不能套pg_table_is_visible的路径遮蔽过滤，也不能要求额外schema USAGE才列关系。排除其他会话临时schema；路径改变不应删除已授权对象行。IS-BASE/ACL/DB。 |
| IS-KIND→tables.table_type | 同一真实relkind及会话临时身份，tables只含r/p/v/f，views仅v | character_data域（底层varchar/1043）；r/p→BASE TABLE、v→VIEW、f→FOREIGN，当前会话临时对象优先LOCAL TEMPORARY。序列、索引、物化视图不列入；原生分区p仍须关系块语义门槛。不支持的种类在创建前拒绝，不能伪造成BASE TABLE。IS-BASE/KIND。 |
| IS-TABLE-EXTRA→self_referencing_column_name、reference_generation、commit_action；user_defined_type_catalog/schema/name；is_typed/is_insertable_into | 前三为PG未提供特性；typed table关联真实reloftype→类型namespace，普通表无typed来源应确认为非typed；可插入性来自关系类别/真实可更新分析 | self_referencing及三项user_defined_type为sql_identifier，其余前两项文本为character_data；前三NULL，非typed表三项类型名NULL且is_typed=NO。两个is_*为yes_or_no域（底层varchar/1043），基表可插入YES，视图须真实分析，不能按当前调用者INSERT权限推导。缺typed/可更新事实的相关投影输出前0A000。IS-COLS/FLAGS。 |
| IS-DEF→views.view_definition | 同一关系OID的PG定义重建，复用视图定义块及pg_get_viewdef，不直接返回原生SelectStmt | character_data域；行可见且当前enabled角色拥有该视图时给语义等价PG SELECT，否则SQL NULL。无owner/角色闭包来源拒绝；已确认非owner才允许权限NULL；owner可读却缺重建能力时0A000，不假NULL。IS-DEF/ACL/LIFE。 |
| IS-VIEW-FLAGS→check_option、is_updatable/is_insertable_into、is_trigger_updatable/is_trigger_deletable/is_trigger_insertable_into | CheckOption的真实等价映射、可更新分析及对应INSTEAD OF触发器事实 | check_option为character_data，NONE/LOCAL/CASCADED；五个is_*为yes_or_no，YES/NO而非bool16。无触发器须先证实对象确无此类触发器才报NO；原生固定NO不是能力证明。缺分析/trigger事实只拒绝相关投影，不伪造所有视图只读。IS-FLAGS。 |

类型域的OID不是硬编码跨构建常量：sql_identifier/character_data/yes_or_no来自本库真实pg_type域身份；底层分别19/1043/1043。按第3章核对域目录及Describe解域后的wire类型（预期19/1043/1043）、name标准长度64、varchar长度−1及typmod（含yes_or_no长度约束），实际RowDescription与域typmod须参考构建采集，不能把域OID、wire OID、列来源OID混同。零行、NULL行仍必须完整描述；缺类型/codec在Parse可判定时拒绝，动态缺授权/定义事实至迟Q或首次Execute成功行输出前0A000。此为产品门禁，不冒称参考PG也应拒绝。

官方依据（PostgreSQL **18**，访问 **2026-10-24**）：[schemata](https://www.postgresql.org/docs/18/infoschema-schemata.html)、[tables](https://www.postgresql.org/docs/18/infoschema-tables.html)、[views](https://www.postgresql.org/docs/18/infoschema-views.html)、[标准模式类型](https://www.postgresql.org/docs/18/infoschema-datatypes.html)定义列与语义；[REL_18_STABLE information_schema.sql](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/catalog/information_schema.sql)核对域底层、逐视图过滤、列权限、临时schema及标志分析，该分支可变。客户端入口依据[psql18手册](https://www.postgresql.org/docs/18/app-psql.html)的SQL与-E，以及[pgJDBC官方Statement/PreparedStatement说明](https://jdbc.postgresql.org/documentation/query/)；后者页面无固定驱动版本，不证明DatabaseMetaData生成过下列SQL。人工探针与客户端实际查询分开，精确18.6构建、DataGrip/ORM查询均未取证。

#### 标准模式列映射

所有字段共用关系OID→pg_attribute的(attrelid,attnum)键，关联pg_class.relnamespace→pg_namespace、atttypid→pg_type；默认以(adrelid,adnum)关联pg_attrdef。行集限定当前PG database的普通表、分区表、视图、外表，排除系统列、删除列和其他连接临时schema；不得从pg_class全部种类机械复制。可见性取真实owner/可用角色闭包或本列SELECT/INSERT/UPDATE/REFERENCES有效权限（含表授权带来的列权限），不按search_path裁剪，不因拥有目录SELECT就暴露全部列。缺权限来源按公共门禁拒绝，不返回伪空集；表可见不等于每列可见。

下表域简写I=sql_identifier、C=character_data、N=cardinal_number、Y=yes_or_no，来自information_schema真实域；底层及预期解域wire OID分别19/1043/23/1043。域OID须从本库类型目录取得，不硬编码；描述、typmod、NULL及零行沿第3章与标准模式表视图块采集，不能将字段值中的UDT OID混作输出字段OID。每行继承上述权限、关联键和当前尚未接线证据。

| 需求→字段（PG域） | 事实来源→目标及不支持策略→验收 |
| --- | --- |
| ISC-ID→table_catalog/schema/name、column_name（I），ordinal_position（N），dtd_identifier（I） | 当前库身份、真实namespace/关系/列名、PG attnum；ordinal从1开始但删除后不压缩存活列位置；dtd只承诺表内类型描述符关联，不作为跨版本稳定ID。原生Offset重排缺口沿列目录块门禁。ISC-BASE/LIFE。 |
| ISC-TYPE→data_type（C），udt_catalog/schema/name、domain_catalog/schema/name（I） | 声明类型身份与域基类型/namespace：内建类型给PG标准名（integer、character varying等），数组ARRAY，其他USER-DEFINED；UDT仍指实际底层类型（int4、varchar、_int4等），域三项只在域列非NULL。不能输出MySQL int/decimal/datetime作为PG身份；缺域/数组来源拒绝相关能力，不能假装text。ISC-BASE/TYPE。 |
| ISC-NULL→is_nullable（Y） | 公开列NOT NULL或域NOT NULL得NO，其余YES，不从默认值、当前数据或客户端权限推断；与pg_attribute及域契约一致。域事实缺失不报YES。ISC-BASE/TYPE/LIFE。 |
| ISC-LEN→character_maximum_length、character_octet_length（N） | PG声明typmod及当前库编码；字符/bit声明上限、字符字节上限分别计算，不适用为NULL。UTF8 varchar(7)为7/28；无界text最大字符长度NULL，但字节上限按参考源码，不误写成一律NULL。缺编码/类型事实拒绝。ISC-SIZE。 |
| ISC-NUM→numeric_precision/radix/scale（N） | 规范OID及typmod；整数/float精度以二进制，numeric以十进制，非数值为NULL，float scale为NULL。不复用MySQL显示宽度；无约束numeric、负scale沿第3章并对参考构建取证，不用0填空。ISC-SIZE/TYPE。 |
| ISC-TIME→datetime_precision（N）、interval_type（C）、interval_precision（N） | PG类型/typmod与interval字段范围；非时间NULL，date精度0，timestamp(3)为3；interval无字段限制时interval_type为NULL。interval_precision为PG不适用的类型化NULL；缺interval身份时拒绝相应能力。ISC-SIZE/TYPE。 |
| ISC-DEFAULT→column_default（C） | 同一pg_attrdef与pg_get_expr语义重建；无默认为SQL NULL，显式DEFAULT NULL是否保留表达式由真实来源确认，不能凭Option None宣称等价；生成列表达式不进入此列。未知表达式拒绝，不能暴露原生SQL或relid前缀。ISC-BASE/DEF。 |
| ISC-IDENTITY→is_identity（Y）、identity_generation/start/increment/maximum/minimum（C）、identity_cycle（Y） | attidentity及列到真实内部序列的i依赖和配置（不是last_value）；YES对应ALWAYS/BY DEFAULT，普通列NO且其余NULL。缺identity身份/关联拒绝，不以AUTO_INCREMENT填充；配置数值为文本。ISC-IDENTITY。 |
| ISC-GEN→is_generated、generation_expression（C），is_updatable（Y） | attgenerated与绑定表达式反解得ALWAYS/表达式，否则NEVER/NULL；普通表列可更新YES，视图/外表须真实列级分析，不等于UPDATE授权。缺分析/表达式不能假NO/NULL。ISC-GEN。 |
| ISC-EXTRA→character_set_catalog/schema/name、scope_catalog/schema/name（I）、maximum_cardinality（N）、is_self_referencing（Y）、collation_catalog/schema/name（I） | 字符集三项、scope三项、最大数组基数均为类型化NULL，self=NO；collation用真实PG身份，默认或不可排序类型三项NULL，显式非默认排序规则给实际库/schema/name。不能用MySQL charset/collation名替代；无法确认是否默认则拒绝该投影。ISC-EXTRA。 |

缺静态字段/codec在Parse/绑定拒绝，动态元数据不充分至迟Q或首次Execute成功输出前0A000；这是产品门禁，不声称PG目标拒绝。不能用未知字段NULL掩盖未实现。官方目标依据：PostgreSQL **18** [columns](https://www.postgresql.org/docs/18/infoschema-columns.html)、[标准域](https://www.postgresql.org/docs/18/infoschema-datatypes.html)及 [REL_18_STABLE information_schema.sql](https://raw.githubusercontent.com/postgres/postgres/REL_18_STABLE/src/backend/catalog/information_schema.sql)的columns视图/精度辅助函数，访问 **2026-10-24**；维护分支可变。客户端入口沿批次16的psql18与[pgJDBC官方查询说明](https://jdbc.postgresql.org/documentation/query/)（访问同日，无固定驱动版本）；本块SQL均为人工探针，不声称DatabaseMetaData、DataGrip或ORM生成过它们。精确18.6构建及运行行为仍未取证。

#### 标准模式约束映射

四视图共享当前database、同一已发布模型快照和真实PG约束身份；内部以约束OID及所属关系OID关联，不按名字猜测。标准输出没有OID，(constraint_catalog,schema,name)在PostgreSQL中仍可能同名跨表，table_constraints与key_column_usage须同时带(table_catalog,schema,name)关联；referential_constraints不含表名，不能在任意数据库中只凭三元名称安全回连。遇歧义必须以pg_constraint的conrelid/confrelid/conindid及真实依赖消歧，保留PG标准视图本身的行集，不能篡改约束名或擅自去重。权限事实沿角色块复用，但不能把pg_constraint的目录枚举可读等同于四视图的行可见。

域简写沿标准列块：I=sql_identifier（底层name/19），C=character_data（varchar/1043），Y=yes_or_no（varchar/1043），N=cardinal_number（int4/23）。域OID从本库pg_type取，Describe解域及typmod沿第3章/标准模式表视图块采集；name标准长度64，varchar−1、int4为4，不能硬编码域OID或把名字输出text/25。零行及NULL仍有完整描述。下表列出四视图全部字段，每行继承上述身份/类型及当前尚未接线证据。

| 需求→视图/字段（PG域） | 来源、关联、目标及不支持策略→验收 |
| --- | --- |
| ISCON-TC-ID→table_constraints.constraint_catalog/schema/name、table_catalog/schema/name（I） | 真实当前库、connamespace/conname、conrelid→pg_class/namespace；仅普通/分区表标准约束，排除触发器/排除约束及其他连接临时schema。独立唯一索引不冒充UNIQUE约束，域约束不套表身份。缺持久来源拒绝。BASE/CROSS/NAME。 |
| ISCON-TC-STATE→constraint_type（C），is_deferrable、initially_deferred、enforced、nulls_distinct（Y） | c/n→CHECK，f→FOREIGN KEY，p→PRIMARY KEY，u→UNIQUE；PG18 NOT NULL亦有真实约束行。三个状态来自condeferrable/condeferred/conenforced，YES/NO不是bool。nulls_distinct仅u取支持索引indnullsnotdistinct的反值，其余SQL NULL；convalidated不等于enforced。缺源不能统一NO/YES，相关DDL副作用前拒绝、既有不可判定投影输出前拒绝。BASE/STATE。 |
| ISCON-KCU→key_column_usage.constraint_catalog/schema/name、table_catalog/schema/name、column_name（I），ordinal_position、position_in_unique_constraint（N） | 仅p/u/f，按conkey展开，列以(conrelid,attnum)取名，排除删除列；ordinal从1起是约束键序而非表列位置。FK第n列取confkey[n]，在conindid父唯一索引indkey中求位置；可与n不同，非FK为NULL。不按列名字典序或直接复制attnum。缺稳定槽位/父索引来源拒绝。BASE/ORDER。 |
| ISCON-RC→referential_constraints.constraint_catalog/schema/name、unique_constraint_catalog/schema/name（I），match_option、update_rule、delete_rule（C） | 每个真实f行；通过FK到父索引再到父p/u约束的真实依赖定位，父conrelid须等confrelid。若FK合法引用独立唯一索引而没有父p/u约束，unique三项应SQL NULL，不能伪造约束名或隐藏FK。MATCH s/f/p映为NONE/FULL/PARTIAL，动作a/r/c/n/d映为NO ACTION/RESTRICT/CASCADE/SET NULL/SET DEFAULT；元数据可表达不证明DDL执行支持PARTIAL等特性，缺源拒绝。BASE/INDEX/STATE。 |
| ISCON-CCU→constraint_column_usage.table_catalog/schema/name、column_name、constraint_catalog/schema/name（I） | c取绑定表达式真实列依赖（去重，不解析字符串猜列）；n取conkey[1]；p/u取本表conkey；f取父表confrelid/confkey，约束schema/name仍是子约束。无ordinal，不可用CCU与KCU的列名或行序配对。常量CHECK无列依赖时确实零行，不代表无CHECK。缺CHECK依赖/NOT NULL身份拒绝相应能力，不填NULL列行。BASE/CHECK。 |
| ISCON-ACL→四视图行过滤（不增加输出字段） | TC/RC：owner可用角色闭包，或表INSERT/UPDATE/DELETE/TRUNCATE/REFERENCES/TRIGGER，或任一列INSERT/UPDATE/REFERENCES；仅SELECT不足。KCU：owner或该列SELECT/INSERT/UPDATE/REFERENCES（含表授权），不因其他列可读就暴露整键。CCU：列所属表owner为当前可用角色；FK检查父表owner，SELECT/写授权均不替代owner。TC/KCU排除其他连接临时schema；RC/CCU按各自官方定义，不能机械复制过滤。列表不受search_path裁剪；未知授权事实拒绝而非伪空集。ACL/NAME。 |

来源缺失、静态字段/codec缺失在Parse/绑定可判定时0A000；动态身份/依赖/授权缺失至迟Q或首次Execute成功行输出前0A000。此为产品能力门禁，不冒称PG合法请求也应拒绝；只有真实无对象/无可见行才可类型正确空集。四视图与pg_constraint在同一对象的身份、列序、动作和状态须一致，目录与标准视图行数可因权限或定义不同而不同。

官方依据（PostgreSQL **18**，访问 **2026-10-24**）：[table_constraints](https://www.postgresql.org/docs/18/infoschema-table-constraints.html)、[key_column_usage](https://www.postgresql.org/docs/18/infoschema-key-column-usage.html)、[referential_constraints](https://www.postgresql.org/docs/18/infoschema-referential-constraints.html)、[constraint_column_usage](https://www.postgresql.org/docs/18/infoschema-constraint-column-usage.html)定义字段与各自可见性；固定[REL_18_0 information_schema.sql](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/catalog/information_schema.sql)核对父索引位置、真实依赖LEFT JOIN、PG18 NOT NULL、权限及临时schema过滤（源码推导）。客户端入口为[pgJDBC官方查询说明](https://jdbc.postgresql.org/documentation/query/)的Statement/PreparedStatement（页面无固定驱动版本）；下述SQL均为人工契约探针，不宣称DatabaseMetaData、DataGrip或ORM生成。精确18.6构建/运行行为仍未取证。

#### 无原生对象的响应策略

每项继承首块完整输入/字段/权限模板。下列字段是本块查询边界；未列PG字段仍须按官方完整关系定义绑定，不能因返回空行忽略未知列。基础类型统一：oid=26、name=19、bool=16、内部char=18、int2=21、int4=23、text=25、text[]=1009、oid[]=1028、aclitem[]=1034、pg_node_tree=194、xid=28；NULL和零行保留原类型，typmod/编码沿第3章。对象OID仅本端关联，不硬编码跨端值。目录枚举权限与对象使用权限分离，均基于真实有效角色；不能因缺授权模型返回伪空集。

| 需求→字段/关联键 | 真实来源、目标策略与验收 |
| --- | --- |
| ABS-SPACE→pg_tablespace.oid/spcname/spcowner/spcacl/spcoptions | 类型26/19/26/1034/1009，owner→角色、关联pg_class.reltablespace。当前无原生PG存储空间事实；明确受限能力允许已知空关系，不捏造pg_default/pg_global或宿主路径。这与PG完整默认tablespace目录有差距，客户端不能依赖默认对象解析。CREATE TABLESPACE/对象显式TABLESPACE请求未开放时副作用前0A000。→EMPTY/DDL。 |
| ABS-FDW→pg_foreign_data_wrapper.oid/fdwname/fdwowner/fdwhandler/fdwvalidator/fdwacl/fdwoptions | 26/19/26/26/26/1034/1009；handler/validator→pg_proc、owner→角色。当前无wrapper/provider，目标补已知类型关系后，在无已安装wrapper事实时空集；不能将未接线误作空集通过。→EMPTY/UNKNOWN。 |
| ABS-SERVER→pg_foreign_server.oid/srvname/srvowner/srvfdw/srvtype/srvversion/srvacl/srvoptions；pg_foreign_table.ftrelid/ftserver/ftoptions | server类型26/19/26/26/25/25/1034/1009，table为26/26/1009；srvfdw→wrapper，ftrelid→pg_class，ftserver→server。无PG FDW对象才空集；原生普通表不得冒充foreign table；存在server时srvtype/version仅在未声明时NULL，ACL的NULL/空数组沿角色块，选项不能暴露秘密。→EMPTY/DDL。 |
| ABS-POLICY→pg_policy.oid/polname/polrelid/polpermissive/polroles/polcmd/polqual/polwithcheck | 26/19/26/16/1028/18/194/194；relid→pg_class，roles→角色（PUBLIC语义沿角色块）。无policy且未启用RLS才可空；不能把已启用RLS但无策略的默认拒绝语义当无RLS。表达式NULL仅按真实未指定语义，非缺模型补NULL；CREATE POLICY/ENABLE ROW LEVEL SECURITY缺能力副作用前拒绝。→EMPTY/DDL。 |
| ABS-TRIGGER→pg_trigger.oid/tgrelid/tgname/tgfoid/tgisinternal | 26/26/19/26/16，关联关系/函数。普通表确无触发器可空；有FK时PG内部约束触发器属于兼容缺口，不可用空集证明完整FK内省。缺真实身份/函数/执行语义时相关内省输出前0A000，不能编造触发器。→EMPTY/REQUIRED。 |
| ABS-REWRITE→pg_rewrite.oid/ev_class/rulename/ev_type/ev_enabled/is_instead/ev_qual/ev_action | 26/26/19/18/18/16/194/194，ev_class→pg_class。无规则普通表可空；已暴露视图时不能把重写目录统一空，须有真实规则身份/定义或在相应查询输出前0A000，视图块的viewdef成功不替代规则契约。→EMPTY/REQUIRED。 |
| ABS-COLL→pg_collation.oid/collname/collnamespace/collowner/collprovider/collisdeterministic/collencoding/collcollate/collctype/colllocale | 26/19/26/26/18/16/23/25/25/25，namespace/owner与声明类型、索引排序身份一致。PG必需内建排序规则不能因无MySQL映射全空；缺匹配的比较语义/locale/provider时请求该能力输出前0A000。libc以外collcollate/ctype可NULL，不能借此隐藏未知provider；MySQL collation ID不直接当PG OID。→REQUIRED/NULL。 |
| ABS-OP→pg_opfamily.oid/opfname/opfmethod/opfowner/opfnamespace；pg_opclass.oid/opcname/opcmethod/opcintype/opckeytype/opcdefault/opcfamily/opcowner/opcnamespace | family类型26/19/26/26/26；class为26/19/26/26/26/16/26/26/26；method→pg_am、类型→pg_type、family→pg_opfamily、owner/namespace→相应目录。内建访问方法及已公开索引必需身份不能全空；不可把原生索引算法名伪装PG family。opckeytype=0仅目标规定的与输入同型含义，非NULL。→REQUIRED。 |
| ABS-OPLINK→pg_amop.oid/amopfamily/amopstrategy/amopopr/amopsortfamily；pg_amproc.oid/amprocfamily/amprocnum/amproc/amproclefttype/amprocrighttype | amop类型26/26/21/26/26；amproc为26/26/21/26/26/26；关联family/operator/proc/type。只对明确不存在的自定义族查询可空；已声明内建族不能缺必要成员。xmin为系统xid/28，不把TSO/int8当PG事务身份，缺源请求xmin须0A000。→REQUIRED。 |

已知对象无行可成功；可选属性确实不适用才SQL NULL；未实现关系/字段/codec或必需事实缺失用能力拒绝0A000（静态可判定在Parse/绑定，动态至迟Q/首次Execute成功行输出前）。真正不存在关系42P01、真正不存在列42703，权限拒绝42501，语法错误42601；合法PG列尚未实现属于0A000，不能误报不存在。失败阶段是产品设计目标，非本轮PG实测。LEFT JOIN空右关系保留左行并产生类型正确NULL，不等于制造一行全NULL的目录对象。

官方依据：PostgreSQL **18**，访问 **2026-10-24**：[tablespace](https://www.postgresql.org/docs/18/catalog-pg-tablespace.html)、[wrapper](https://www.postgresql.org/docs/18/catalog-pg-foreign-data-wrapper.html)、[server](https://www.postgresql.org/docs/18/catalog-pg-foreign-server.html)、[foreign table](https://www.postgresql.org/docs/18/catalog-pg-foreign-table.html)、[policy](https://www.postgresql.org/docs/18/catalog-pg-policy.html)、[trigger](https://www.postgresql.org/docs/18/catalog-pg-trigger.html)、[rewrite](https://www.postgresql.org/docs/18/catalog-pg-rewrite.html)、[collation](https://www.postgresql.org/docs/18/catalog-pg-collation.html)、[opfamily](https://www.postgresql.org/docs/18/catalog-pg-opfamily.html)、[opclass](https://www.postgresql.org/docs/18/catalog-pg-opclass.html)、[amop](https://www.postgresql.org/docs/18/catalog-pg-amop.html)、[amproc](https://www.postgresql.org/docs/18/catalog-pg-amproc.html)给出字段/关联/NULL语义；[错误码](https://www.postgresql.org/docs/18/errcodes-appendix.html)给出上述SQLSTATE名称。客户端官方依据为[psql 18](https://www.postgresql.org/docs/18/app-psql.html)的-E/echo-hidden及目录命令，后续采集`\db`、`\dFd`、`\dFs`、`\dO`、`\d public.abs_t`；本轮未运行，未声称这些命令生成下节SQL。DataGrip模板版本及官方查询出处仍未采集。精确18.6参考构建和行为未取证。

#### 目录求值与普通查询边界

复用首块字段模板、第2章名称/CTE遮蔽、第3章固定类型/codec、第4章函数工厂及第5章事务/同步契约。以下是目标，现有适配器的有界支持不提升为完整PG SQL承诺。

| 需求→输出字段/关联键 | 来源、类型、权限、目标与拒绝策略 |
| --- | --- |
| ROUTE-JOIN→c.oid、n.nspname；c.relnamespace=n.oid | 共享infoschema/model的真实表/库身份经pg_oid，类型oid/26、name/19；目录枚举及schema可见性沿关系/命名空间/权限块。INNER按键匹配，LEFT空右边保留左行及带类型NULL，不造对象；多库身份不能混接。→J。 |
| ROUTE-CTE/SUB→x.oid、标量c.oid、array_agg(c.oid) | 同一Execute目录快照，CTE输出继承oid/26，标量零行NULL、超过一行须拒绝；oid数组为oid[]/1028，不能用text替代，空输入聚合目标NULL而非伪空数组。CTE无持久对象OID，权限沿源关系，不因包一层绕过。数组codec未实现则输出前0A000。→C/S/A。 |
| ROUTE-PARAM→$1::oid筛选c.oid | 参数为协议输入，不生成元数据事实；ParameterDescription=26，结果c.oid=26，NULL仍固定类型。Parse确定类型、Bind校验数量/格式/值，Execute用绑定值过滤真实目录；Q含未绑定参数不得回退共享SQL。→P。 |
| ROUTE-MIX→c.oid、u.id | c来自目录；u.id来自真实用户表，按第3章声明类型，不能猜测OID或假装目录provider。含目录和普通关系的整条查询在接入共享planner完整支持前0A000；静态可判定在Q分析/扩展Parse，动态至迟首次Execute成功行输出前。拒绝涵盖JOIN、逗号FROM、CTE及子查询，不拆成两次执行或返回部分成功。→M。 |
| ROUTE-ORDINARY→u.id、字面量 | 无真实目录依赖的查询走共享名称解析/优化/执行链；字符串或注释中的目录字样不接管，CTE及本地同名对象沿第2/5章遮蔽。系统函数只按注册与类型契约接线，未知/未实现能力显式拒绝，不能以目录缓存代替用户数据。→O。 |

当前硬界：每个SELECT最多8个JOIN、查询嵌套8层、全查询16个CTE、每CTE最多24列；UNION列表最多16个附加arm（不把它误写成总共16个SELECT）；表达式深度64、谓词预算128、CASE分支32。累计物化上限16384行、comparison计数上限100000由Execution共享于CTE/子查询；它不是最终返回行数或所有CPU操作计数，LIMIT 1不能保证不触限。comparison/materialize检查取消，不能由检查点推导响应时延。当前越限返回ConnError::Session，不在本块虚报已测SQLSTATE。

目标保留显式、可诊断的有界限制：解析边界0A000，执行资源越限采用明确资源错误（目标54000，实际映射待实现取证），取消目标57014；禁止截断成功、缓存部分行或取消后重试到普通引擎。上限是本地安全策略，无PG标准数值依据；调整须依真实客户端探针、规模/内存/取消基准另立门槛。通用GROUP BY/HAVING、任意聚合、递归/写入CTE、相关子查询及未开放数组形态先拒绝；必要探针逐项接线，共享内核承担通用执行，不能把此evaluator扩成第二套执行器。

官方依据：PostgreSQL **18**，访问 **2026-10-24**：[表表达式](https://www.postgresql.org/docs/18/queries-table-expressions.html)定义JOIN/子查询关系，[WITH](https://www.postgresql.org/docs/18/queries-with.html)定义CTE作用域与求值，[协议流程](https://www.postgresql.org/docs/18/protocol-flow.html)定义Parse/Bind/Describe/Execute、Sync与取消；类型及错误码沿第3章和前块官方依据。[psql 18手册](https://www.postgresql.org/docs/18/app-psql.html)的-E/echo-hidden用于后续采集真实目录SQL，不证明下节人工SQL由客户端生成。精确18.6构建与运行行为未取证，客户端版本/SQL仍未采集。

#### 目录更新与连接隔离

复用首块模板及关系/列/权限/可见性块，不重定义字段语义。CAT-REFRESH要求一个目录查询的所有provider、JOIN、CTE及定义函数来自同一已选目录版本；后续空闲态查询在DDL成功发布后取得新事实。共享已提交DDL应被同库连接看见，连接私有path、当前库、prepared/portal与权限上下文不得泄露。这里的单次目录一致性是产品目标，不声称PG所有内部目录访问均等同普通MVCC查询。

| 字段/查询输出 | 真实来源、关联键、PG目标类型/OID | 权限/可见性、目标与缺口策略 |
| --- | --- | --- |
| R的c.oid、c.relname、c.relnamespace | 同一SchemaRef中TableInfo.id、库ID经pg_oid映射；namespace键关联pg_namespace.oid；oid/26、name/19、oid/26 | 同库具CONNECT、schema USAGE与目录读权限的owner夹具；不按path删除显式目录行。改名只变名称；删建换身份。当前name出口text/25差异沿关系块；无可靠身份拒绝，不能造OID。 |
| C的a.attrelid、a.attnum、a.attname、a.atttypid、a.atttypmod、a.attnotnull | 同版TableInfo/ColumnInfo，attrelid关联c.oid，(attrelid,attnum)列键，类型关联pg_type.oid；依次oid/26、int2/21、name/19、oid/26、int4/23、bool/16 | 列位置/声明类型/typmod/删除槽位遵守列块，不能把持久列ID当attnum。普通可见列过滤attisdropped；drop槽位不复用目标需真实模型支持，当前Offset差异不能删验收。缺事实/codec按列块门禁，不用NULL或旧值掩盖。 |
| D的current_database()；prepared R/C描述与数据 | 连接context的SELECT DATABASE()与Execution.database；name/19（当前text/25差异沿数据库块）；R/C固定投影类型由metadata/第3章提供 | 每连接启动库独立；库不存在/不可用须按数据库生命周期明确报错，不回退其他库。不能用快照缺失返回伪成功空集；具体诊断待采。描述固定不代表行固定，实际描述改变须重分析或明确拒绝。 |

事务外DDL完成后，R/C须完整反映一次发布结果；多条DDL之间允许看到中间的已发布版本。并发查询可见完整旧版或完整新版，不能将新版表名拼上旧版列。已执行portal续读保持原结果；新portal重新执行获取适用新版本。prepared普通SELECT结果结构改变按第3/5章重新分析/失效门槛，不能默默发送与Describe不同的行；当前不一致拒绝只证明局部保护，精确PG诊断/发生于Bind还是Execute须参考端采证。

事务型DDL未开放时严格沿第5章“事务型建表能力门槛”早拒绝、无发布/隐式提交；未来开放后须补本连接未提交可见、其他连接无脏读、COMMIT发布与ROLLBACK无残留，不将每次domain.info_schema()直接解释为已满足。长事务和REPEATABLE READ下的目录特殊可见性、锁等待单列参考取证，不承诺每次查询都无条件见最新。

官方目标来源（PostgreSQL 18，访问2026-10-24）：[PREPARE](https://www.postgresql.org/docs/18/sql-prepare.html)用于DDL/path引发重新分析及会话私有prepared；[事务隔离](https://www.postgresql.org/docs/18/transaction-iso.html)用于已提交观察边界，不能替代目录特殊行为实测；[pg_class](https://www.postgresql.org/docs/18/catalog-pg-class.html)、[pg_attribute](https://www.postgresql.org/docs/18/catalog-pg-attribute.html)用于字段及关联；客户端官方[psql 18](https://www.postgresql.org/docs/18/app-psql.html)的-E用于后续抓取内省SQL。以下均为人工契约探针，不声称psql生成过；精确客户端版本、18.6参考构建及运行行为均未取证。

#### 命令行客户端内省流程

沿首块证据模板，客户端能力分成 CLI-PSQL 与 CLI-LIBPQ；连接成功仅证明启动链。所有查询必须分别登记完整 SQL、位置参数/格式、描述、行、权限和元数据变化，真实客户端生成 SQL 与下方人工探针分别保存。

| 查询字段/需求 | 真实来源、关联键及 PG 目标类型/OID | 权限/可见性、当前证据、目标和不支持策略 |
| --- | --- | --- |
| L 的 oid、datname | infoschema 库 ID 经 namespace_oid；oid/26、name/19；oid 对应数据库身份，非列的类型 OID | 可连接专用库且可读目录的 owner；数据库行不按当前 search_path 隐藏，CONNECT 与目录可读性分开。数据库块定义 owner/ACL 等其他字段；现有 libpq DATABASES_SQL 将 oid 强转 bigint/20、名称为 text/25，不能混作本投影目标。缺身份不得造 0 或空集。 |
| R 的 oid、nspname、relname、relkind | TableInfo.id 经 table_oid；relnamespace=n.oid，nspname 来自库/schema 映射；依次 oid/26、name/19、name/19、"char"/18 | 显式 public 的目录查询与 psql 无限定模式列表的 path 可见性分别验收；沿关系/命名空间/可见性块。现有 libpq 关系测试 oid/26，名称和 kind 为 text/25，是已知类型差异。未提供必要关系不得假空集。 |
| C 的 attrelid、attnum、attname、atttypid、atttypmod、attnotnull | TableInfo/ColumnInfo 同版事实；attrelid=c.oid，(attrelid,attnum)列键，atttypid=pg_type.oid；依次 oid/26、int2/21、name/19、oid/26、int4/23、bool/16 | owner、schema USAGE、目录读权限；沿列块的声明类型/删除槽位/非空规则。不由行值推类型；空表仍有完整列描述。当前客户端测试未覆盖本 C 全投影，待新增，不能由关系测试推断。 |
| psql \l、\dt public.cli_probe、\d public.cli_probe 的全部实际投影 | 官方客户端 describe.c 生成并按版本/服务版本分支，涉及库、关系、列、定义、注释、权限及格式函数；逐条 SQL/每列按首块登记到上述字段块及索引/约束/默认/注释/权限块 | 目前原始 SQL 未采，输出字段和 OID 未取证，不能用 L/R/C 冒充实际生成查询。必要投影/函数缺失时记录分析或 Execute 的能力拒绝 0A000 与恢复，不能全局返回空集换取命令完成；目标合法流程仍为成功。 |

name/19、"char"/18 的描述和 codec 门槛引用第 3 章及本章相应字段块，typlen/typmod/表 OID/属性号逐列记录，不能只比较文本。权限缺失沿角色权限块，不伪造 owner/ACL；合法空用户库应没有用户关系但保留系统目录与描述，权限拒绝不能解释为库空。

官方目标依据（版本 PostgreSQL 18，访问 2026-10-24）：[psql 手册](https://www.postgresql.org/docs/18/app-psql.html)用于元命令及 -E；[libpq 执行和结果接口](https://www.postgresql.org/docs/18/libpq-exec.html)用于参数、描述、NULL 与结果状态；[官方 REL_18_STABLE describe.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/bin/psql/describe.c)仅为生成查询的后续定位入口，可变分支不等于已锁定客户端构建。字段类型和目录关联的官方依据复用数据库/关系/列块。本轮无精确 psql/libpq 补丁版本及 PostgreSQL 18.6 参考构建，18.6 精确行为保持未取证。

#### 驱动元数据内省流程

沿首块模板，每次调用必须分别登记服务器返回列和驱动合成 ResultSet，二者的列名、PG 类型/OID及 NULL 可不同；JDBC Types.INTEGER=4 等代码是数据，不能充当 RowDescription 的类型 OID。下表的字段组引用本章数据库、命名空间、关系、列、类型、索引、约束、默认表达式、注释、权限与可见性块中的逐字段事实源及拒绝规则；同组每个字段仍须在后续实采清单逐项展开。

| 调用/必要请求结构（源码证据，非实跑 SQL） | 字段来源、关联键及服务器 PG 类型/OID | 目标、权限/可见性与缺口策略 |
| --- | --- | --- |
| getSchemas | pg_namespace.nspname 为 name/19；current_database() 为 name/19。namespace_oid 关联真实库/schema；临时 schema 排除条件涉及 current_schemas(true)、下标及 replace | 按 TABLE_SCHEM 排序，pattern 是 LIKE；hideUnprivilegedObjects 开启时需真实 has_schema_privilege。无临时对象不造行；不能因缺正则/函数而返回所有 schema 或空集。 |
| getTables | c.relnamespace=n.oid；d.objoid=c.oid、objsubid=0、classoid=pg_class 身份；数据库/schema/关系名称 name/19，CASE TABLE_TYPE、注释、常量投影 text/25；关系 relkind 原字段为 char/18 | 表类型由真实 relkind/schema 分类，types 数组筛选；TABLE_TYPE、TABLE_SCHEM、TABLE_NAME 排序。无注释是 NULL，驱动 SQL 中空字串常量保留为空字串；权限筛选沿真实 ACL，不能造 owner。 |
| getColumns | a.attrelid=c.oid、a.atttypid=t.oid；def.(adrelid,adnum)=(a.attrelid,a.attnum)，注释关联沿源码；名称 name/19，atttypid/typbasetype oid/26，attnotnull bool/16，atttypmod/typtypmod int4/23，attlen int2/21，row_number 结果 int8/20，nullif(identity/generated,'')为 char/18、定义/注释 text/25，typtype char/18 | 先过滤 schema/table、正 attnum 与未删除槽位，再窗口编号，最后过滤 column pattern；按 schema/table/编号排序。类型精度、默认、非空、自增/生成列来自真实声明，不能由行值或名称猜测；domain/identity 未实现按已有块拒绝。 |
| getPrimaryKeys | ct.oid=a.attrelid=i.indrelid，ci.oid=i.indexrelid，ct.relnamespace=n.oid；i.indisprimary bool/16，indkey int2vector/22，indnkeyatts int2/21；_pg_expandarray 输出 n int4/23、x int2/21；名称 name/19 | schema/table 为精确等号，展开索引键序并排除 INCLUDE；按 table_name、pk_name、key_seq 排序。PK_NAME 是索引 ci.relname，不能改用臆造约束名。真实无主键才零行；复合字段/集合函数不支持需明确拒绝。 |
| getImportedKeys | conrelid/confrelid → 两侧 class，conkey/confkey int2[]/1005 同序下标 → 两侧 attribute；conindid → 被引用索引；generate_series 位置 int4/23；两端名称 name/19，规则/deferrability CASE int4/23；confupdtype/confdeltype char/18、condeferrable/condeferred bool/16 | 精确 schema/table；按被引用 schema/table、约束名、序号排序。前置 getMaxIndexKeys 查询 pg_settings.setting text/25，值来自真实能力。没有 FK 才零行，存在但未映射不能假空集；更新/删除与延迟规则沿约束块。 |
| getIndexInfo | ct.oid=i.indrelid，ci.oid=i.indexrelid，ci.relam=am.oid；indisunique/indisclustered bool/16，amname name/19，indoption int2[]/1005；展开 n int4/23；reltuples float4/700、relpages int4/23；pg_get_indexdef / pg_get_expr / 注释 text/25 | unique=true 只取唯一索引；按 NON_UNIQUE、TYPE、INDEX_NAME、ORDINAL_POSITION 排序。approximate 在此版本方法未改变 SQL；不能据此承诺精确统计。无统计真实来源按索引块策略拒绝所需投影，禁止造 cardinality/pages；不适用 predicate 的 NULL 与缺实现分开。 |
| getTypeInfo | t.typnamespace=n.oid，typrelid → class.oid；typname name/19、t.oid oid/26，typrelid oid/26；排除 toast 和普通表复合行类型，权限选项可能追加 has_type_privilege | 真实内建类型来自第 3 章/类型块；驱动 TypeInfoCache 还可触发额外类型查询并合成 serial 等行，不能将其当服务器新增 OID；按 JDBC DATA_TYPE 排序。所需类型不能全空，缺类型映射/缓存请求须登记分析或执行拒绝。 |

官方来源（访问 2026-10-24）：[pgJDBC REL42.7.8 PgDatabaseMetaData.java](https://github.com/pgjdbc/pgjdbc/blob/REL42.7.8/pgjdbc/src/main/java/org/postgresql/jdbc/PgDatabaseMetaData.java) 的同名方法、getImportedExportedKeys/getMaxIndexKeys 与 Field/tuple 定义用于版本化请求及驱动转换；[同 tag TypeInfoCache.java](https://github.com/pgjdbc/pgjdbc/blob/REL42.7.8/pgjdbc/src/main/java/org/postgresql/jdbc/TypeInfoCache.java) 为附加类型请求/映射入口；[PostgreSQL 18 系统目录](https://www.postgresql.org/docs/18/catalogs.html)为字段目标入口；[pg_attribute](https://www.postgresql.org/docs/18/catalog-pg-attribute.html) 与[会话信息函数](https://www.postgresql.org/docs/18/functions-info.html) 用于列属性及 current_database/current_schemas 类型复核，其他具体字段依据沿本章对应对象块。源码审阅发现的版本分支/客户端实现差异以该 tag 为准，不等于已验证 PostgreSQL 18.6。

完整服务器生成 SQL、Parse 参数 OID/格式、结果格式、所有初始化及 TypeInfoCache 补充 SQL **尚未线端取证**。下节给完整 Java 方法输入及一条源码可还原请求；其余请求以上述官方方法为精确定位，禁止将此表或手写同形 SQL 标为实际驱动日志。每个投影的 typlen/typmod/表 OID/属性号及实际 NULL 描述待参考端逐列采集；服务器字段目标沿对应字段块，驱动合成结果沿本 tag 的 Field/tuple 定义。

### 范围外

#### 目录执行链与证据模板

本块不补全部目录对象/字段、information_schema、ACL/定义函数、客户端矩阵或求值语法，不实现多 schema、目录缓存失效、事务型 DDL、系统函数注册或通用数组 codec；后续块按本模板承接。目标覆盖仍需逐项填满，不能因当前 provider 有限而缩减客户端需求。不得将 PG 目录直接写入、完整系统目录或扩展加载列为本轮承诺；不运行 Cargo、真实服务、客户端或差分平台。

当前 namespace provider 只输出 pg_catalog 和映射当前原生数据库的 public；不能将本地所有数据库冒充 PG 同一数据库内的 schema。字段缺事实不能一概空集；下列示例的必需 oid/name 不允许 SQL NULL 或空集替代，未来 owner/ACL 需独立真实来源，不在本块伪造。目录按名称列出对象不等于该角色拥有 USAGE/CREATE，search_path 查找结果也不等于目录行过滤规则。

#### 数据库目录映射

只定义数据库枚举、五个字段及 DATABASES_SQL 的附属投影，不实现 owner/角色子系统、数据库模板复制、禁用连接开关、ACL、encoding/locale、完整 psql 列表或 DataGrip/JDBC/ORM 兼容，也不修生产代码。psql::listAllDbs 的额外字段是后续需求，不能因当前有限字段就宣称 psql 数据库列表完整可用。不得伪造 postgres/template0/template1、PG 内部 owner 或注释；原生系统库如何分类必须后续取证后明确，不直接作为可连接 PG 库承诺。未知/不存在数据库的目录筛选零行、缺失必需事实的能力拒绝、连接阶段找不到库是三类情形，不用一个空列表遮掩；连接诊断细节由第 7 章实采。

#### 命名空间目录映射

本块仅定义 pg_namespace 四字段、库/schema 层次、关联及验收，不实现用户 schema、角色/ACL 子系统、事务 DDL、临时 schema、系统函数注册或完整 psql/JDBC/DataGrip/ORM 流程。information_schema 的视图字段留标准模式块；它作为 namespace 的真实身份仍是完整列表目标的依赖，不能凭空补一行就宣称其对象可用。psql 的描述和可能附带的 publication 查询留客户端块取证。schema 列表与有效搜索路径、目录可读与对象可用分别验证；不得用类型正确的空集掩盖真实存在的用户 schema，也不得把当前 NULL owner、缺失 ACL 字段宣称兼容。

#### 关系身份与稳定标识

不实现 OID 分配/代际库、持久 schema/owner/索引名称、完整分区/继承/TOAST/物化视图、序列运行状态、ACL或全 pg_class 字段；索引/列/约束/序列/角色的详细契约交各自后续块。保留范围内外键一致性要求，不以未实现为由删减目标。普通名称、类型/codec、函数注册、事务与 wire 继续由第2–5/7章负责。只列下述待新增/待运行验收，不修改 Rust、不运行 Cargo/Go、差分平台或真实客户端。

#### 列目录与声明类型

只定义列声明目录字段与生命周期，不实现持久PG属性槽位、identity序列、生成表达式、系统列、索引/序列属性及完整类型系统；不修改普通SELECT类型推导。pg_type全字段、默认表达式反解、ACL、information_schema.columns及真实客户端流程交对应窄块。上述缺口须登记门槛，不以空集或固定false宣称覆盖；本轮不修改生产代码、不运行Cargo或客户端。

#### 内建类型目录映射

不实现类型注册、通用数组/vector运算、域/复合/枚举/扩展类型系统，不补函数/角色/排序规则子系统，不修改第3章coercion或扩大到全量JDBC元数据流程。目录身份闭环、真实来源和明确拒绝仍属目标，不能通过删类型或降低目标范围获得文档验收。固定类型声明与普通结果身份分叉是既有缺口，后续按窄任务处理；本轮不改Rust、Cargo或测试。

#### 索引目录与定义反解

本块只定义索引目录和反解的文档契约，不实现索引DDL、表达式索引、部分索引、INCLUDE、访问方法、opclass/collation注册或在线回填，不扩建执行器。约束反解/外键交批次8；真实默认表达式、权限、可见性、刷新、psql/JDBC/DataGrip流程交各自块。第2/3/5章及关系/属性块的缺口不在此改代码。

合法复杂PG索引目标不删减：没有真实保真来源时，创建请求在副作用前0A000；原生侧已有前缀/非等价方法等对象在目录输出或反解前0A000，不隐藏为不存在或返回删去条件的CREATE INDEX。确实无索引的关系才允许类型正确空集；真实普通索引没有表达式/谓词才允许对应SQL NULL。provider当前会枚举当前库其他表，WHERE限定一个OID不保证隔离无关坏模型，此限制需在后续求值块独立验证；本轮不改变其行为。

#### 约束目录与定义反解

本块只写pg_constraint/pg_get_constraintdef契约，不实现约束DDL、延迟检查、历史数据验证、FK级联、CHECK绑定器、PG节点树或操作符/域/继承/分区/时态子系统，不改第2–5章，不运行Cargo或真实客户端。NOT NULL在PG18也有contype=n行，排除/触发器/域约束亦不能从“本块只取p/u/f/c”推断完整目录已覆盖；相应来源与拒绝门槛交后续窄任务，不删减PG目标。information_schema约束视图交批次18。

现有独立唯一索引来源混同、CHECK令provider全库失败、缺验证/延迟标记及不完整父模型校验仅登记，不扩大为代码修复。既有坏对象被WHERE排除也不保证查询成功，有界求值块另验；既有不可反解对象不得偷偷丢行。副作用、即时/延迟执行、提交/回滚与失败恢复引用第5章，第7/8章承担wire和真实客户端运行，不用本轮文档验收宣称实现通过。

#### 默认表达式目录契约

只定义默认/生成表达式目录事实及反解契约；不实现表达式树、默认OID分配器、information_schema视图、生成列执行器、序列/identity、函数工厂或客户端适配。继承/分区默认覆盖、域默认与任意PG内部树的完整序列化另行细分；本块必需定义缺失须明确拒绝。当前整库扫描导致无关坏默认使查询失败是待实施缺口，本任务不扩展修复。

#### 视图定义内省契约

不实现视图DDL、持久绑定树、名称重写、重启恢复、pg_views/information_schema.views、权限引擎或物化视图刷新。物化视图定义属于函数目标，当前缺真实接线须记录并拒绝，不把它当普通View伪造；CHECK OPTION执行、安全屏障/security_invoker与任意查询转换另建实施任务。不得为补这些缺口扩大本轮代码范围。

#### 序列身份与依赖契约

不实现序列分配、identity DDL/DML、OWNED BY 存储、通用依赖图或权限系统；不将本块扩展到分区/扩展对象的全部依赖。不执行 Cargo、差分平台或真实客户端；第5章分配/回滚和第7章wire能力仍需各自验证。当前拥有边空集是明确元数据缺口，不代表 identity 已实现或所有 PG 依赖应为空。

#### 函数目录与注册边界

不实现CREATE FUNCTION/PROCEDURE、扩展加载、过程语言、聚合/窗口/集合返回执行器或ACL子系统；不扩大全局函数注册表，不修改第4章。pg_proc字段空缺、类型不符和缺普通SQL链是后续实施输入，本任务不修代码；不以目录三个原语代表完整内建函数全集。

#### 对象注释与描述契约

不实现COMMENT解析/持久化、目录函数、角色体系或属性槽位，不补其他对象描述生态。缺数据库、角色、表空间来源逐项沿上节门禁，不能以全空目录掩盖表/列已有来源。真实客户端抓取、18.6参考构建与协议执行留后续实施。

#### 角色权限与所有权契约

不实现PG认证、角色DDL、授权引擎、owner持久化、RLS、SECURITY DEFINER或完整ACL运算；不把原生MySQL角色语义自动等价为PG语义。不运行Cargo/Go、实际客户端或差分平台，不修改第1–5章及其他块；本块只定义所需安全事实及缺失门禁。其他对象目录、名称可见性与客户端完整流程沿各自块。

#### 可见性与名称遮蔽契约

本轮只定义可见性/名称遮蔽契约，不实现用户schema、权限系统、临时对象、函数注册或prepared失效基础设施。pg_operator/opclass/opfamily、collation/conversion、统计对象及全文检索的可见性函数仍属后续对象块：未开放时绑定0A000，禁止恒真占位；不因官方存在这些函数就声称完整支持。第2/5章拥有解析与会话事务规则，本块消费其契约；真实客户端完整查询、wire错误字段与性能交第7/8章取证。

#### 标准模式表视图映射

不实现information_schema路由、用户schema、域codec、授权存储、可更新视图、触发器或typed table；不把其他标准视图（columns/constraints/privileges等）吞入本块。原生MySQL标准模式与PG标准模式须按方言隔离，不能全局重命名原生schema/catalog字段。本块只定义目标、拒绝门槛及后续验收，不声称真实客户端或18.6已通过。

#### 标准模式列映射

仅定义columns的声明、默认、权限、identity/generated字段和验收，不实现标准视图路由、PG域存储、排序规则、列墓碑、授权、序列或生成列表达式；复用已有契约，未开放分支保留明确门禁。其他标准视图及完整客户端工作流留对应块，不扩大为实现任务；本轮无Cargo、构建、lint或客户端实测。

#### 标准模式约束映射

本块只定义四个标准约束视图字段、关联及验收，不实现provider、DDL、FK执行、延迟/验证、CHECK依赖图、授权存储或完整客户端流程。约束/索引/列持久身份与pg_depend缺口沿既有专块另拆实施，不递归补齐子系统；不改原生MySQL information_schema语义，不改第1–5章，不运行Cargo/Go或差分平台。

#### 无原生对象的响应策略

不实现provider、类型codec、FDW/RLS/触发器/规则/排序规则/访问方法，不补完整子系统；pg_operator/pg_aggregate、继承与分区沿各自函数/关系契约，不用本块统一空集覆盖其缺口。不采集真实客户端或运行Cargo/Go、18.6差分。当前空provider与目标必需对象冲突只在本块登记，不重写前序契约或降低目标。

#### 目录求值与普通查询边界

不实现路由器、provider、共享planner桥接、普通聚合/递归/写入执行器或codec；不将目录局部函数白名单当全局函数注册。不扩大其他目录字段、跨连接更新及客户端兼容范围（分别交既有块、批次21–25）。资源数值不是性能承诺；本轮不运行Cargo/Go、真实客户端或PG参考实例。

#### 目录更新与连接隔离

本块不实现DDL、快照/锁/缓存失效，不修改provider或测试，不运行Cargo、Go或真实客户端。schema版本不作为PG新增目录字段对外暴露；不扩成全库对象生命周期或全部隔离级别支持。权限、OID、删除槽位及name codec缺口引用对应块，事务型DDL归第5章，wire/portal完整生命周期归第7章，客户端实际查询归后续客户端块。并发删库、角色撤权和临时对象需后续单独取证，不能由无连接缓存推出已安全。

#### 命令行客户端内省流程

本块仅定义 psql/libpq 最小真实目录流程与证据交接，不实现命令行驱动、完整 psql 元命令、客户端全部版本兼容或通用目录执行器。认证/TLS/取消/协议帧归第 7 章，真实运行与产物归第 8 章；JDBC/DataGrip/ORM 各自取证。不得为运行探针补建权限、类型或目录子系统；现有缺口作为后续实施输入。

#### 驱动元数据内省流程

不实现 JDBC 目录适配、正则/窗口/集合返回/复合字段求值、类型缓存或权限子系统，不升级或移植驱动，不运行 Java/Cargo/Go 或 DataGrip UI。不把本地 JDBC 手写 SQL 流程扩写成 DatabaseMetaData 已通过；真实驱动执行与两端差分产物交第 8 章。18.6 精确构建、线端 SQL 和 OID 未采不阻塞本块文档交付，但持续作为实现验收门槛。

### 验收

#### 目录执行链与证据模板

下面是模板的完整人工查询实例 `CAT-CHAIN-NAMESPACE/pg`（PG 专属），用例 `catalog.chain.namespace.base` / `.fresh` / `.reject` 均**待新增、未运行**；不是声称已抓取的 psql/DataGrip 查询。客户端目标为 psql 18.x，实际二进制/驱动精确版本未采集；扩展复验用 wire 夹具并记录其构建 SHA。AsterSQL 与真实 PG 各连接自己的可丢弃数据库，UTF-8、空闲 I，先执行 `SET search_path TO public` 并记录实际数据库/角色名；角色有 pg_namespace SELECT；fresh 另需当前库 CREATE、持有自己创建的 schema。库/角色由夹具管理，不在 SQL 中嵌入凭证。

base 完整 SQL（参数 []，Q，文本结果）：

```sql
SELECT n.oid AS oid, n.nspname AS nspname
FROM pg_catalog.pg_namespace AS n
WHERE n.nspname = 'public'
ORDER BY n.oid;
```

初态要求夹具存在且仅有一个名为 public 的 schema；目标恰好一行 `(O_public, 'public')`，两列均非 SQL NULL；O_public 为本端真实对象身份，不硬编码为 PG 默认值，也不要求两端用户对象 OID 数值相等。列序 oid/nspname；oid 类型 oid/OID26、typlen4，nspname 类型 name/OID19，name 长度随参考构建 NAMEDATALEN 采证；typmod、来源表 OID/属性号和完整描述按第 3/7 章采集，当前缺证不可跳过。成功 SQLSTATE 不适用，完成标签目标 SELECT 1、末尾 I。

| 需求→输出字段 | 真实来源与关联键 | 当前静态证据 | PG 目标与缺失策略→验收 |
| --- | --- | --- | --- |
| namespace 身份→n.oid AS oid | pg_catalog.rs 的目录 provider 取 Execution.snapshot.AllSchemas() 中当前原生 database 的 schema.id，以 pg_oid::namespace_oid 映射 public；pg_catalog 的 11 是内建身份。目标关系键为 pg_namespace.oid，供 pg_class.relnamespace 等引用；本查询不执行 JOIN。 | CatalogQuery::column 为该字段使用 OID_TYPE，namespace_row 写 Signed 值；无运行证据。当前原生 database→public 映射仅是受限适配。 | oid/OID26，非 NULL、稳定且在有效范围；对象存在却无快照/合法身份时拒绝，目标执行阶段 0A000，无行无副作用，当前错误映射未实采；不得造 0 或空集。base/fresh 验证身份及更新。 |
| namespace 名称→n.nspname AS nspname | 同一 provider 当前把上述原生对象命名为 public，非任意 PG schema 名称事实；目标应取真实 schema 名称，关联同一 oid，并保留第 2/5 章引号身份。 | column 使用原生文本 code253，pg_result::pg_type 映射 text/OID25；与目标 name/OID19 存在静态差距，不能声称类型通过。 | name/OID19，非 NULL，需真实身份和正确 codec；缺 codec 的开放前门禁目标 Describe/结果编码阶段 0A000，无成功行，Q 在输出前拒绝；不得暗中改成 text。base 及扩展 Describe 比较，零行也固定类型。 |

扩展 base 另开新连接/夹具：命名 Parse `cat_ns_base` 使用同一 SQL、参数 OID []；Bind `cat_ns_portal` 参数 []、文本结果，分别 Describe statement/portal、首次 Execute 全量、Sync，目标描述与 Q 同型、相同行值、Sync 后 I。两种协议分别保留完整描述/行/命令/错误字段；不是只有连接成功或行数相等。fresh 使用同一两列查询，仅将 WHERE 字符串替换为 `'cat_contract_ns'`，完整输入为：

```sql
SELECT n.oid AS oid, n.nspname AS nspname
FROM pg_catalog.pg_namespace AS n
WHERE n.nspname = 'cat_contract_ns'
ORDER BY n.oid;
```

fresh 初态确保该名不存在，A/B 独立连接同库同角色，各查零行且描述同型；A 先 Parse/Bind/Describe fresh 查询但不 Execute，B 单独 Q 执行 `CREATE SCHEMA cat_contract_ns` 并完成到 I，A 首次 Execute/Sync 目标看到一行 `(O_new, 'cat_contract_ns')`。随后 A/B 新查询均同一 O_new，重复读取不重分配；B `DROP SCHEMA cat_contract_ns` 后，两端新查询都零行且描述不变。已执行 portal 的续取生命周期另归第 7 章，不要求它刷新成新查询。另一数据库的独立连接执行同查询仍零行；另一 PG 连接改变 path 不得改变本查询的限定关系。目录行存在不证明 USAGE，对受限角色的对象访问与 ACL 留权限块另验。

当前 provider 没有任意 schema 的上述来源，本例 fresh 为目标缺口，不能把 CREATE SCHEMA 改写成 CREATE DATABASE 来算通过；若能力门禁拒绝建 schema，保存真实拒绝阶段/诊断及两连接零行、无残留，并标 fresh 未验证而非成功更新。夹具未满足 SELECT/CREATE、独立数据库或类型门槛同样记录缺口。清理两连接先 Sync/ROLLBACK 至 I，再由所有者按需 `DROP SCHEMA cat_contract_ns`（仅在夹具确认已创建时），关闭语句/portal/连接并回收夹具；清理失败不可记通过。不在本块扩建 schema 子系统。

reject 完整 SQL `SELECT n.unknown FROM pg_catalog.pg_namespace AS n`，参数 []，初始 I/目录 SELECT 权限；产品有界实现的目标为 Q 分类后的字段绑定拒绝、0A000、无行无元数据修改，随后 `SELECT 1 AS ok` 返回 1、I。扩展在 Parse 阶段拒绝，经 Sync 后再执行恢复查询。源码测试 catalog_binding_has_explicit_support 只覆盖同类 pg_database 未知列，此 namespace 变体须待新增；PG 参考端按自身未知列错误单列采证，不强求与产品 0A000 相同。错误 S/V/C/M 及适用 D/H/P、精确文本/位置仍未采。此拒绝目标不替代未来开放字段的 PG 名称错误契约。

人工覆盖审阅：需求→oid/name 真实来源与关联→目标类型/行→base 描述/取值、fresh 元数据变化/隔离、reject 阶段/恢复均有落点；当前静态差距明确保留。文档验证不包含任何当前行为通过结论，有效运行用例数 0。

#### 数据库目录映射

以下 `catalog.database.*` 用例全部**待新增、未运行**，有效运行用例数 0。完整值/描述/错误字段按前块模板记录；SQL 为人工契约探针，只有 DB-LIST 原文来自仓库 DATABASES_SQL，真实客户端生成 SQL 及精确版本未采集。PG/AsterSQL 各用自己的可丢弃数据库集合、UTF-8、初始 I、参数 []、Q 文本结果，角色 cat_reader 有目录 SELECT；库 cat_db_a、cat_db_b 为普通非模板且允许连接，分别由已存在的真实角色 cat_owner 拥有，cat_reader 仅能连接 a。控制角色有创建/删库/改库/授权权限，所有管理 SQL 单条自动提交执行，禁止在事务内建删库。夹具必须核实这些前提；AsterSQL 缺任一前提记能力缺口，不把近似 MySQL 授权当 PG 测试通过。

DB-BASE 完整 SQL：

```sql
SELECT N.oid, N.datname, N.datdba, N.datistemplate, N.datallowconn
FROM pg_catalog.pg_database AS N
WHERE N.datname IN ('cat_db_a', 'cat_db_b')
ORDER BY N.datname;
```

目标两行 `(O_a,'cat_db_a',R_owner,f,t)`、`(O_b,'cat_db_b',R_owner,f,t)`，类型依次 oid/26、name/19、oid/26、bool/16、bool/16，均非 NULL。所有 O/R 为本端真实身份，不硬编码 PG 默认 OID。cat_reader 从 a 读取仍见 b，b 的 datallowconn 仍为 t；尝试新连接 b 因 CONNECT 权限拒绝，不允许用目录 false 代替。参考端准备与清理 SQL 为控制角色依次 `CREATE DATABASE cat_db_a OWNER cat_owner`、`CREATE DATABASE cat_db_b OWNER cat_owner`、`REVOKE CONNECT ON DATABASE cat_db_b FROM PUBLIC`、`REVOKE CONNECT ON DATABASE cat_db_b FROM cat_reader`、`GRANT CONNECT ON DATABASE cat_db_a TO cat_reader`，夹具保证 reader 非 owner/超级用户且无继承 CONNECT。角色创建与凭据由安全夹具管理，不写入查询记录。读取权限不足另存参考诊断，不记作字段缺失。

DB-LIST 使用以下 DATABASES_SQL 原文（参数 []），在 a 与另一个允许连 b 的 owner 连接分别执行：

```sql
select N.oid::bigint as id,
       datname as name, D.description, datistemplate as is_template,
       datallowconn as allow_connections,
       pg_catalog.pg_get_userbyid(N.datdba) as "owner"
from pg_catalog.pg_database N
left join pg_catalog.pg_shdescription D on N.oid = D.objoid
order by case when datname = pg_catalog.current_database() then -1::bigint else N.oid::bigint end
```

仅在夹具确认无数据库注释且没有造成 objoid 误连的其他共享对象描述时，目标每个真实数据库恰一行；首行为连接当前库，余行 OID 升序，列类型依次 int8/20、name/19、text/25、bool/16、bool/16、name/19，description 为 NULL，普通库 f/t，owner 为真实角色名称。其他 PG 自带库保留其真实事实，不要求全表只有 a/b，也不要求两端全表同名；以夹具创建库子集逐项比较。原 SQL 缺 classoid 限定须登记安全关联缺口，不能因当前描述空集认定通用 JOIN 正确；后续描述块另测不同共享对象 OID 冲突。缺真实 owner 时此列表目标拒绝 0A000，不以 NULL owner 成功替代。

DB-OWNER 额外完整 SQL `SELECT N.datdba, pg_catalog.pg_get_userbyid(N.datdba) AS owner FROM pg_catalog.pg_database N WHERE N.datname = 'cat_db_a'`；目标 `(R_owner,'cat_owner')`，oid/26、name/19。owner/reader 两连接所得一致，改变登录角色不能改变对象 owner；当前 NULL/int8 与 NULL/text 是静态差距。未知角色 OID 的函数行为交权限/系统函数块，不把现有返回 NULL 的测试当真实 owner 支持。

DB-EMPTY 完整 SQL `SELECT N.oid, N.datname, N.datdba, N.datistemplate, N.datallowconn FROM pg_catalog.pg_database N WHERE N.datname = 'cat_db_absent'`，夹具保证不存在该库；零行但保持 BASE 五列类型、SELECT 0、末尾 I。另开新连接到 cat_db_absent 应启动阶段拒绝，不产生成功目录行；精确诊断/SQLSTATE 未采，交第 7 章取证。DB-BASE/LIST/EMPTY 的扩展复验均用各自完整 SQL：Parse 命名语句、参数 OID []；Bind 命名 portal、参数 []、文本结果；Describe statement/portal；Execute 全量、Sync。列序、类型、值与各自 Q 相同，零行也固定描述；name/owner 缺能力的拒绝不得记作成功描述。

DB-FRESH 完整 SQL `SELECT N.oid, N.datname FROM pg_catalog.pg_database N WHERE N.datname = 'cat_db_fresh' ORDER BY N.oid`，两列 oid/26、name/19。连接 A 先 Parse/Bind/Describe 但未 Execute；管理连接 B 单条 `CREATE DATABASE cat_db_fresh` 后完成至 I，A 首次 Execute/Sync 目标一行新 OID/名称；A/B 的新查询 OID 相同且重复稳定。B 单条 `DROP DATABASE cat_db_fresh` 后两端新查询零行、描述不变；其他既有库 OID 不变。对已执行 portal 的续取不要求重建快照。改变 A 的 `SET search_path TO pg_catalog` 后同一限定查询及当前库身份不变；B 路径不随 A 改变。当前 schema_snapshot 来源和已有测试意图提供静态依据，不证明上述两连接目标已运行。

DB-POLICY：在 B 以管理角色执行 `ALTER DATABASE cat_db_b ALLOW_CONNECTIONS false`，A 重新执行 BASE 目标 b 的 datallowconn=f、datistemplate=f，reader 与 owner 均不得新连 b；恢复 `ALTER DATABASE cat_db_b ALLOW_CONNECTIONS true` 后 b=t，reader 仍因缺 CONNECT 被拒绝。另执行 `ALTER DATABASE cat_db_b IS_TEMPLATE true`，BASE 目标仅 b 的 datistemplate=t；恢复 `ALTER DATABASE cat_db_b IS_TEMPLATE false`，不创建 template0/template1。产品若未开放上述策略/模板语义，在变更执行前 0A000、无元数据变化，保持相应 PG 完整目标未验证；普通库受限 false/true 不能算策略变更已支持。

DB-REJECT 完整 SQL `SELECT N.unknown FROM pg_catalog.pg_database N`：产品当前有界字段绑定目标 0A000、无行无副作用；Q 分类/绑定拒绝，扩展 Parse 拒绝后 Sync，再 `SELECT 1 AS ok` 应返回 1/I。PG 参考端未知字段诊断单独采证，不要求相同 0A000。权限/连接及产品能力门禁均记录 S/V/C/M、适用 D/H/P、确切阶段和原 SQL，未实采不杜撰错误文本。

清理时先关闭可能连接待删库的连接与 portal，控制连接保持在独立管理库；恢复 b 的两个策略，再按夹具实际创建记录单条 `DROP DATABASE cat_db_fresh`、`DROP DATABASE cat_db_b`、`DROP DATABASE cat_db_a`（已删除者不重复），回收授权及自建角色。任一前提、变更、类型、权限或清理不满足须保留诊断与未验证项，不缩为只验行数。

#### 命名空间目录映射

能力 `CAT-NAMESPACE/pg` 的下列 `catalog.namespace.*` 用例均为**待新增、未运行**。AsterSQL 与 PG 各由管理员准备可丢弃数据库 ns_doc_a/ns_doc_b；A、B 连接 a，C 连接 b，UTF-8，默认 public，初态 I。本组管理员为夹具专用超级用户，拥有两库 CREATE、schema/角色管理能力；另有互无角色继承的 ns_owner、ns_reader，reader 有两库 CONNECT、pg_namespace/pg_class SELECT。角色/库由夹具创建和回收，不在 SQL 内嵌凭证。所有语句逐条发送，除以下注明扩展参数外均参数 []、Q、文本结果；不在显式事务内准备 DDL。

完整人工基础探针 QN：

```sql
SELECT n.oid AS oid, n.nspname AS nspname
FROM pg_catalog.pg_namespace AS n
WHERE n.nspname IN ('public', 'pg_catalog', 'ns_user', 'ns_private', 'ns_renamed')
ORDER BY n.nspname;
```

| 用例、字段需求与完整输入/顺序 | 目标、当前限制和副作用检查 |
| --- | --- |
| NS-BASE：仅 public/pg_catalog 初态，A/B/C 各 QN；A `SET search_path TO pg_catalog`、QN、`SET search_path TO ''`、QN、`SELECT current_schema()`；B QN；A `RESET search_path` | QN 各恰两行 pg_catalog/public，oid/name 均非 NULL，列类型26/19；A 路径切换不改变行/OID，空路径 current_schema 为 SQL NULL。同库 A/B 的对应 OID 相等，跨库 public 只比较本端身份关系，不能硬编码 PG public OID。当前名称为 text/25 是已知差距；当前库缺失不得把仅系统行当通过。 |
| NS-EMPTY：A `SELECT n.oid,n.nspname FROM pg_catalog.pg_namespace n WHERE n.nspname='ns_missing'`；另查 `SELECT n.oid,n.nspname FROM pg_catalog.pg_namespace n WHERE n.nspname='public' LIMIT 0` | 均零行，保留两列26/19描述，目标 SELECT 0/I；与无法取得真实元数据而拒绝区分。 |
| NS-FRESH：a 管理员 `CREATE SCHEMA ns_user AUTHORIZATION ns_owner`；A/B QN；随后 `ALTER SCHEMA ns_user RENAME TO ns_renamed`；A/B QN；`DROP SCHEMA ns_renamed`；A/B QN；再次 `CREATE SCHEMA ns_user AUTHORIZATION ns_owner` 后 QN | 行数2→3→3→2→3，改名保留该对象 OID，旧名称消失；删除后旧关联不可残留，重建重新取真实身份。无无关 schema 或库变化。当前没有库内用户 schema 来源，门禁须在改变元数据前拒绝并保留原两行；它是能力限制，不能标为该目标通过。 |
| NS-DB：a/b 管理员分别 `CREATE SCHEMA ns_user AUTHORIZATION ns_owner`；A/B/C QN；只在 a `ALTER SCHEMA ns_user RENAME TO ns_renamed`；A/B/C QN | 两库各有自己的 public/ns_user，无同库重复 namespace；a 改名仅 A/B 变化，C 的 ns_user/OID 不变。不把原生库 ns_doc_b 作为 a 的 schema。当前仅 public 桥接不足以通过，本组与 FRESH 独立重置夹具。 |
| NS-JOIN：a `CREATE SCHEMA ns_user AUTHORIZATION ns_owner` 后 `CREATE TABLE public.ns_same(id integer)`、`CREATE TABLE ns_user.ns_same(id integer)`；A `SELECT n.oid AS namespace_oid,n.nspname,c.relname,c.relnamespace FROM pg_catalog.pg_namespace n JOIN pg_catalog.pg_class c ON c.relnamespace=n.oid WHERE c.relname='ns_same' ORDER BY n.nspname` | 两行 public/ns_user，每行 namespace_oid=relnamespace，namespace OID 不同，关联来自本端真实对象；列类型26/19/19/26。两 schema 中同名表不合并，不误连另一库。同名 schema 跨库允许；另在本组已有 ns_user 后重复 `CREATE SCHEMA ns_user AUTHORIZATION ns_owner` 应失败，QN/OID不变，具体 PG 诊断实采。当前无 ns_user 支持，不能用两个原生 database 的表模拟成功。 |
| NS-OWNER：a `CREATE SCHEMA ns_user AUTHORIZATION ns_owner`；A 与 reader 各 `SELECT n.oid,n.nspowner FROM pg_catalog.pg_namespace n WHERE n.nspname='ns_user'`；管理员 `ALTER SCHEMA ns_user OWNER TO ns_reader` 后重复查询 | 两列26/26，owner 非 NULL，关联真实角色 OID；转移前后 namespace OID 不变，两个读者观察同一 owner 变化。缺 owner 来源时当前 NULL/int8 不能过关，应按目标门禁；不运行时不猜角色 OID。 |
| NS-ACL：a `CREATE SCHEMA ns_private AUTHORIZATION ns_owner`；管理员 `REVOKE ALL ON SCHEMA ns_private FROM PUBLIC`、`REVOKE ALL ON SCHEMA ns_private FROM ns_reader`、`CREATE TABLE ns_private.ns_secret(id integer)`、`INSERT INTO ns_private.ns_secret VALUES (7)`、`GRANT SELECT ON ns_private.ns_secret TO ns_reader`；reader QN 及 `SELECT n.nspacl FROM pg_catalog.pg_namespace n WHERE n.nspname='ns_private'`，再 `SELECT id FROM ns_private.ns_secret`；管理员 `GRANT USAGE ON SCHEMA ns_private TO ns_reader`，reader 重复两条 SELECT；管理员 `REVOKE USAGE ON SCHEMA ns_private FROM ns_reader` 后再重复 | 无 USAGE 时 reader 仍可枚举 ns_private 和读取 ACL（字段类型1034），但读表须权限拒绝，非零行成功；授予后读表恰 [(7)]，撤销后再拒绝，schema 行/OID不变、ACL反映实际授权、表数据仍7。不要逐字符硬编码 ACL 顺序；解析 grantee/grantor、USAGE/CREATE 与授权选项，分别记录 SQL NULL/空数组。独立重置后管理员 `CREATE SCHEMA ns_private AUTHORIZATION ns_owner`，在无 ALTER DEFAULT PRIVILEGES 的夹具中 reader 运行同一 ACL 投影，目标 SQL NULL；不能把上述 REVOKE 后状态当默认 NULL。当前 ACL 未绑定，返回0A000属限制，不能用 NULL 通过。 |

所有用户 schema 组互相独立重置，只有前一步成功才进入后续 SQL；任一能力门禁后先记录失败，再在 I 下重查 QN、当前库与路径，确认无半建对象/库泄漏；T/E 中按第 5 章先 ROLLBACK，扩展先 Sync。清理先关闭 portal/statement、ROLLBACK/RESET；管理员只清理本组实际创建的对象，完整清理 SQL 为 `DROP TABLE public.ns_same`、`DROP TABLE ns_user.ns_same`、`DROP TABLE ns_private.ns_secret`，再按存在性执行 `DROP SCHEMA ns_user`、`DROP SCHEMA ns_renamed`、`DROP SCHEMA ns_private`（不用 CASCADE 掩盖残留）；关闭 A/B/C/reader 后由夹具删除两库及专用角色。

扩展复验 NS-BASE/EMPTY 使用完整 SQL `SELECT n.oid,n.nspname FROM pg_catalog.pg_namespace n WHERE n.nspname=$1::text ORDER BY n.oid`，Parse 参数 OID [25]，Bind 单个文本 UTF-8 public（另例 ns_missing），参数格式0、结果格式[0,0]，Describe statement/portal、Execute max_rows=0、Sync；预期一行/零行、两列26/19。NS-FRESH 另在 CREATE 前 Parse QN（参数[]），CREATE 后新 portal 首次 Execute 需见新增行；改名/删除后各新 Bind/Execute 同 statement 复验，不用旧 portal 缓存充当新快照。重复 NS-OWNER/ACL 的无参数投影分别收集26/26及1034描述；二进制只在 codec 能力已确认后另验，ACL 不因请求格式1而假定支持。

逐例记录数据库/角色、完整版本/构建、原始 SQL/参数、行/NULL、列序与 OID/typlen/typmod/可空性/格式/来源属性、命令标签、错误原文/SQLSTATE/阶段/位置及 I/T/E。未知 wire/诊断保留待采，成功 SQLSTATE 不适用。客户端需求对应 psql `\dn`（name/owner）及 `\dn+`（ACL/描述），后续用该版本 `-E` 抓取实际查询和附属请求；本人工 SQL 不替代该流程，18.6 精确行为、真实 psql/JDBC/DataGrip/ORM 结果均未验证。

#### 关系身份与稳定标识

以下全部为**待新增/待运行**人工契约用例，复用公共证据模板；已有测试仅供实现时扩展。独立夹具建两个空数据库 rel_doc_a/b、专用角色 rel_owner/rel_reader，管理员授予连接、public USAGE及目录 SELECT；A/B均连a、C连b，初始I，UTF-8、search_path=public（隐式pg_catalog优先），Q参数[]、结果文本。管理员在a执行 `CREATE TABLE public.rel_t(id integer)`、`CREATE INDEX rel_ix ON public.rel_t(id)`、`CREATE VIEW public.rel_v AS SELECT id FROM public.rel_t`、`CREATE SEQUENCE public.rel_s START WITH 10`；b独立执行同名四条。夹具初始化若门禁即记录该缺口，不继续虚构行。

基础查询 QR 为 `SELECT c.oid,c.relname,c.relnamespace,c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE n.nspname='public' AND c.relname IN ('rel_t','rel_ix','rel_v','rel_s','rel_renamed') ORDER BY c.relname`。四列目标26/19/26/18，typmod=-1、非NULL，typlen=4/64/4/1（name长度绑定构建）；所有下列查询沿用相同权限/格式，另述参数除外。

| 验收 ID与完整操作 | 期望行、类型/变化或拒绝 |
| --- | --- |
| REL-BASE：A/B/C QR；A `SELECT c.oid,c.relnamespace FROM pg_catalog.pg_class c WHERE c.oid='pg_catalog.pg_class'::regclass::oid`；A `SELECT c.oid,c.relname,c.relnamespace,c.relkind FROM pg_catalog.pg_class c WHERE c.relname='rel_missing'`及 `SELECT c.oid,c.relname,c.relnamespace,c.relkind FROM pg_catalog.pg_class c WHERE c.relname='rel_t' LIMIT 0` | QR恰4行，名字/类别依次rel_ix/i、rel_s/S、rel_t/r、rel_v/v，各自外键命中本库public；A=B，同库无重复OID，C不混入a的事实（不要求跨数据库用户OID不同）。系统查询恰(1259,11)，两列26；两个空结果保留26/19/26/18描述、SELECT 0。 |
| REL-KEY：A `SELECT i.indrelid,i.indexrelid,t.oid,x.oid FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class t ON t.oid=i.indrelid JOIN pg_catalog.pg_class x ON x.oid=i.indexrelid WHERE t.relname='rel_t' AND x.relname='rel_ix'`；`SELECT s.seqrelid,c.oid FROM pg_catalog.pg_sequence s JOIN pg_catalog.pg_class c ON c.oid=s.seqrelid WHERE c.relname='rel_s'`；`SELECT a.attrelid,c.oid FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid WHERE c.relname='rel_t' AND a.attname='id'` | 各1行，四列/两列均26，同端indrelid=t.oid、indexrelid=x.oid、seqrelid=c.oid、attrelid=c.oid；表OID≠索引OID。附属字段行为留对应窄块，不把 JOIN 成功当定义/序列功能已完备。 |
| REL-LIFE：记录A/B QR中的表OID=T、索引OID=X；管理员 `ALTER TABLE public.rel_t RENAME TO rel_renamed`，A/B QR；`DROP VIEW public.rel_v`、`DROP TABLE public.rel_renamed`，A/B QR；`CREATE TABLE public.rel_t(id integer)`、`CREATE INDEX rel_ix ON public.rel_t(id)`，A/B QR；关闭连接并以同一持久数据重启服务、重连再QR | 改名保留T及X，旧表名消失；删除后T/X均无行且QR只剩序列；重建后QR3行，新表/索引OID均不同于T/X，序列OID不变；A/B每步一致，C每步QR仍原4行。重启不重新分配剩余身份；不能用内存domain重建替代持久服务重启。依第5章只验成功提交后可见，未开放DDL应在副作用前门禁。 |
| REL-CAST：A `SELECT c.oid,'public.rel_t'::regclass::oid,c.oid::regclass,c.oid::regclass::text FROM pg_catalog.pg_class c WHERE c.relname='rel_t'` | 1行类型26/26/2205/25，前两值相等，后两显示在当前path下均可重新解析到该OID。持久表达式中的早绑定与运行时text查找分开交第4/5章，目录现有执行时查找不证明早绑定。 |
| REL-NAME：独立重建基础四对象夹具后 `CREATE TABLE public."Rel Mixed"(id integer)`、`CREATE TABLE public.pg_class(marker integer)`；A依次 `SET search_path=public,pg_catalog`、`SELECT 'pg_class'::regclass::oid,'pg_catalog.pg_class'::regclass::oid,'public."Rel Mixed"'::regclass::oid`；再 `SET search_path=pg_catalog,public` 并重复SELECT；另在a建 `CREATE TABLE public.rel_other(id integer)`，尝试 `CREATE INDEX rel_ix ON public.rel_other(id)` | 两次SELECT类型26/26/26，第一次首列为用户pg_class的OID、第二列1259；第二次前两列均1259，带引号对象OID不变。重复rel_ix在同库同schema失败且无新索引；跨库同名允许由基础四对象夹具独立验收。已存在原生局部重名不得任选一项；碰撞及引号错误的精确参考诊断待采。 |
| REL-UNKNOWN：独立空夹具确认 `SELECT c.oid FROM pg_catalog.pg_class c WHERE c.oid=4294967295::oid` 零行后，`SELECT '4294967295'::regclass,'4294967295'::regclass::oid,NULL::regclass`；`SELECT 'public.rel_missing'::regclass`；`SELECT pg_catalog.to_regclass('public.rel_missing')`；`SELECT '"unfinished'::regclass` | 数字探针1行(4294967295,4294967295,NULL)，列2205/26/2205，不新增目录行；缺名cast须错误，to_regclass为单行NULL/2205；坏引号须输入错误。现有独立测试断言意图为缺名42P01、坏输入22P02，精确参考端诊断/阶段待采，不能写已通过。未注册to_regclass时产品门禁0A000，不能作为NULL目标通过。 |
| REL-OWNER：A与reader `SELECT c.oid,c.relowner FROM pg_catalog.pg_class c WHERE c.relname='rel_t'`；管理员 `ALTER TABLE public.rel_t OWNER TO rel_owner` 后重复；管理员 `REVOKE ALL ON TABLE public.rel_t FROM PUBLIC`、`REVOKE ALL ON TABLE public.rel_t FROM rel_reader`，reader再投影并 `SELECT id FROM public.rel_t` | owner查询1行26/26、真实owner非NULL，转移前后关系OID不变，两角色看到同一owner。无对象SELECT仍可读目录，但读表须权限错误而非空集成功；缺owner来源请求字段输出前0A000。角色/ACL细节及精确诊断交权限块。 |
| REL-KIND：独立夹具 `CREATE TABLE public.rel_p(id integer) PARTITION BY RANGE(id)`、`CREATE TABLE public.rel_p0 PARTITION OF public.rel_p FOR VALUES FROM (0) TO (10)`、`CREATE INDEX rel_p_ix ON public.rel_p(id)`；`SELECT c.oid,c.relname,c.relkind FROM pg_catalog.pg_class c WHERE c.relname IN ('rel_p','rel_p0','rel_p_ix') ORDER BY c.relname` | 目标3行rel_p/p、rel_p0/r、rel_p_ix/I，类型26/19/18；需真实PG分区/索引模型支持。当前原生partition标记与物理分区未成独立关系构成缺口，应在不支持DDL的副作用前0A000，不能把当前p/I行当本组通过。 |
| REL-RANGE：待扩展独立映射测试，对namespace/table分别输入0、i64::MIN/MAX，索引输入(0,1)、(1,i64::MAX)，并比较(42,1)/(43,1)/(42,-1)；独立元数据夹具注入越界ID后QR；另 `SELECT '4294967296'::oid` | 映射均显式失败、三个合法pair彼此不同且不碰系统OID；元数据越界为产品执行门禁0A000且零成功行，无截断/漏行/部分结果，精确wire诊断待采。oid字面量越界与元数据能力门槛分开，现有测试意图22003，参考端实测待补。 |

扩展复验用 `SELECT c.oid,c.relname,c.relnamespace,c.relkind FROM pg_catalog.pg_class c WHERE c.relname=$1::text ORDER BY c.oid`，Parse参数OID[25]，Bind文本UTF-8 rel_t（另例rel_missing），参数格式0、结果格式[0,0,0,0]；Describe statement/portal、Execute max_rows=0、Sync，预期1/0行及26/19/26/18描述。REL-LIFE另在改名前Parse此SQL，改名/删除/重建后各用新portal Bind/Execute，参数分别rel_renamed、rel_renamed、rel_t，对照T/零行/新T；旧portal缓存不当作新快照。REL-CAST/UNKNOWN各用完整原SQL、参数[]复验2205/NULL/错误；binary仅在对应codec开放后另采，不能从文本通过推定。

逐例采集公共模板要求的版本/构建、角色/库/path、原SQL/参数/格式、每列完整描述、行/命令标签/错误原文及阶段、I/T/E恢复；成功SQLSTATE不适用，错误精确字段未采保留缺证。reader不得拥有表、无继承角色或默认授权；C验证跨库隔离与A/B验证同库稳定分开。每组独立重置；失败后按第5章ROLLBACK/扩展Sync并重查QR确认无半建对象。清理关闭portal/statement、RESET search_path；管理员按实际存在性执行 `DROP VIEW public.rel_v`、`DROP TABLE public.rel_t`或`DROP TABLE public.rel_renamed`、`DROP SEQUENCE public.rel_s`、`DROP TABLE public."Rel Mixed"`、`DROP TABLE public.pg_class`、`DROP TABLE public.rel_other`；分区组先 `DROP TABLE public.rel_p0` 再 `DROP TABLE public.rel_p`，索引随所属表删除；各库分别清理后关闭连接，由夹具删除专用数据库/角色。不用CASCADE掩盖关联残留。客户端需求对应psql关系列表和描述，实际版本须用-E另采；本轮未运行客户端及18.6参考端。

#### 列目录与声明类型

以下 `ATTR-*` 全部为**后续待新增/待运行**，不是本轮结果。统一为同一测试库、UTF8、空闲事务、owner连接、search_path=public,pg_catalog；第二连接使用同库且具备上述目录SELECT、无对象SELECT的角色。准备/清理DDL由owner执行；所有输入为人工探针，客户端版本及18.6构建待采。Q参数[]、文本结果；扩展查询用下列参数版本，Parse OID=[26]，Bind值为本端关系OID的十进制文本、格式[0]，结果分别[0]/[1]，Describe statement/portal、Execute、Sync，不以Describe执行数据查询。

```sql
CREATE TABLE public.attr_contract (
  id integer NOT NULL, label varchar(24), amount numeric(12,3),
  obsolete integer, stamp timestamp(3) without time zone
);
SELECT a.attrelid,a.attnum,a.attname,a.atttypid,a.atttypmod,
       a.attnotnull,a.attisdropped,a.attidentity,a.attgenerated
FROM pg_catalog.pg_attribute a
WHERE a.attrelid='public.attr_contract'::regclass::oid AND a.attnum>0
ORDER BY a.attnum;
-- 参数版完整SQL：
SELECT a.attrelid,a.attnum,a.attname,a.atttypid,a.atttypmod,
       a.attnotnull,a.attisdropped,a.attidentity,a.attgenerated
FROM pg_catalog.pg_attribute a WHERE a.attrelid=$1 AND a.attnum>0
ORDER BY a.attnum;
```

| 用例→需求 | 期望行/描述、真实变更及拒绝验收 |
| --- | --- |
| ATTR-BASE/KEY→所属关系/名字/类型 | 5行同一attrelid=本端pg_class.oid；位置1..5，名称依次id/label/amount/obsolete/stamp，OID依次23/1043/1700/23/1114，typmod依次-1/28/786439/-1/3；仅id的attnotnull=true；所有attisdropped=false、两个模式空串。输出描述依次26/21/19/26/23/16/16/18/18，长度4/2/64/4/4/1/1/1/1（name标准构建前提），字段自身typmod=-1。两端用户关系OID按身份关联，不比较固定数值。第二角色与path改变后限定查询行集合相同；另一库同名表不得泄漏。 |
| ATTR-TYPE→修饰符/声明而非结果 | 执行 `ALTER TABLE public.attr_contract ALTER COLUMN label TYPE varchar(7);`、`ALTER TABLE public.attr_contract ALTER COLUMN amount TYPE numeric(8,2);` 后同一参数重执行，位置不变，typmod分别11/524294。另执行 `SELECT pg_catalog.format_type(1043,28),pg_catalog.format_type(1700,786439);`，参数[]，目标一行text/25两列，值character varying(24)、numeric(12,3)；当前无FROM形式的路由不由该目标探针推断已支持。无约束numeric/未声明时间精度/负scale按第3章另建声明夹具，不以原生默认精度冒充显式声明。缺保真元数据应在输出前0A000。 |
| ATTR-NULL→非空变化 | `ALTER TABLE public.attr_contract ALTER COLUMN label SET NOT NULL;` 后仅label的attnotnull变true，随后 `ALTER TABLE public.attr_contract ALTER COLUMN label DROP NOT NULL;` 回false；当前表无数据避免填充争议。prepared重执行读新快照，Describe的输出列类型保持不变。 |
| ATTR-LIFE→顺序/改名/删除/新增 | `ALTER TABLE public.attr_contract RENAME COLUMN label TO "Label";` 后位置2保留、大小写保留。`ALTER TABLE public.attr_contract DROP COLUMN obsolete;`、`ALTER TABLE public.attr_contract ADD COLUMN added varchar(7);` 后未过滤查询为6槽位：4为dropped=true/atttypid=0，不硬编码其内部名；stamp仍5、added为6。再执行 `SELECT a.attnum,a.attname FROM pg_catalog.pg_attribute a WHERE a.attrelid='public.attr_contract'::regclass::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum;`，参数[]，描述21/19，活列位置1/2/3/5/6。无持久槽位时DROP/ADD须在DDL副作用前0A000，不能以当前原生重排结果验收PG目标；既有重排状态输出前拒绝。 |
| ATTR-IDENTITY→模式及真实关联 | owner执行 `CREATE TABLE public.attr_identity (a integer GENERATED ALWAYS AS IDENTITY,b bigint GENERATED BY DEFAULT AS IDENTITY,c integer);`，基准SQL将关系名替换为attr_identity；三行OID23/20/23，typmod均-1，attnotnull=true/true/false，attidentity=a/d/空串，attgenerated均空串。同一SQL关联第5章真实序列/生成验收，不能由AUTO_INCREMENT夹具替代。缺模式或关联时CREATE副作用前0A000、无残留表/序列；既有对象缺源在目录输出前拒绝。 |
| ATTR-GENERATED→stored/virtual | `CREATE TABLE public.attr_generated (x integer,gs integer GENERATED ALWAYS AS (x+1) STORED,gv integer GENERATED ALWAYS AS (x+2) VIRTUAL);`，基准SQL替换关系名，三行OID23/typmod=-1，attgenerated=空串/s/v，attidentity均空串；表达式及读写行为另验，不仅查模式。缺其中一种语义则相应声明DDL副作用前0A000，无残留对象，不能改成普通列或另一生成模式。 |
| ATTR-EMPTY/ERROR→描述与拒绝 | 参数版Bind值"0"/OID26应零行但保持上述完整描述；对真实无用户列关系，仅在确有该类对象时验证零行。缺ModelMeta、非法/重复Offset、缺类型、未支持生成列或所请求系统/索引属性分别记录实际执行拒绝阶段，产品目标0A000且无成功行；不能因WHERE只选某表就声称provider不枚举其他表。错误字段/文本待实采，协议恢复与事务恢复分开沿第5/7章。 |

清理（owner，独立语句，参数[]）：`DROP TABLE IF EXISTS public.attr_generated;`、`DROP TABLE IF EXISTS public.attr_identity;`、`DROP TABLE IF EXISTS public.attr_contract;`。所有查询/DDL须保存参考端及产品端的完整版本、输入、行值、描述、错误、角色权限与前后快照；prepared在DDL前Parse/Describe，DDL提交后新Bind/Execute的刷新，与已启动portal缓存分开。本轮只有文档/源码审阅，不宣称以上查询已可执行；Ready文档检查通过不能替代行为验收。

#### 内建类型目录映射

以下`TYPE-*`均为**后续待新增/待运行**的人工契约探针；只有JDBC分隔符查询明确源于上述42.7.8官方源码，其产品端参数形式作如下适配。公共前提：同一测试库、UTF8、空闲事务、owner连接、search_path=public,pg_catalog、目录SELECT；第二连接同库具备目录SELECT但无用户表SELECT，权限实际授予证据须保留。Q参数[]、text结果；每段SQL逐条发送。扩展参数查询在DDL前Parse、Describe statement，Parse OID=[26]，Bind值为指定本端OID的十进制文本、格式[0]，分别结果格式[0]/[1]，Describe portal、Execute、Sync。所有输出比较列序、OID/typlen/typmod和text/binary值，零行仍保留描述；协议恢复沿第7章。

```sql
-- TYPE-BASE，Q参数[]；全量限定投影不能只比较非空。
SELECT t.oid,t.typname,t.typnamespace,t.typtype,t.typcategory,
       t.typelem,t.typarray,t.typdelim,t.typlen,t.typbyval,t.typisdefined
FROM pg_catalog.pg_type t WHERE t.oid IN (23,25,1007,1009) ORDER BY t.oid;
-- TYPE-ARRAY，扩展参数：先23，再25；反向查询依次1007、1009。
SELECT t.oid,t.typname,t.typelem,t.typarray FROM pg_catalog.pg_type t WHERE t.oid=$1;
-- TYPE-JDBC，源查询的JDBC ? 改为wire $1；绑定1007、1009。
SELECT e.typdelim FROM pg_catalog.pg_type t,pg_catalog.pg_type e
WHERE t.oid=$1 AND t.typelem=e.oid;
-- TYPE-CLOSURE，准备DDL由owner执行；全部提交后查询。
CREATE TABLE public.type_contract (i integer,v varchar(24),n numeric(12,3));
SELECT a.attnum,a.atttypid,t.oid,t.typname,t.typnamespace,
       pg_catalog.format_type(a.atttypid,a.atttypmod) AS declared_type
FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type t ON a.atttypid=t.oid
WHERE a.attrelid='public.type_contract'::regclass::oid AND a.attnum>0
  AND NOT a.attisdropped ORDER BY a.attnum;
-- 同一完整参数查询用于TYPE-FORMAT/UNKNOWN，Parse OID=[26,23]。
SELECT pg_catalog.format_type($1,$2) AS type_name;
```

| 用例→需求 | 期望行/描述、元数据变化及拒绝证据 |
| --- | --- |
| TYPE-BASE→身份/类别/长度 | 4行，OID23/25/1007/1009，name=int4/text/_int4/_text，namespace11；typtype均b、category=N/S/A/A、typelem=0/0/23/25、typarray=1007/1009/0/0、typdelim均逗号、typlen=4/-1/-1/-1、typbyval=true/false/false/false、defined均true。输出描述26/19/26/18/18/26/26/18/21/16/16，长度4/64/4/1/1/4/4/1/2/1/1（name标准构建前提）。第二角色同样可见；path改变后限定查询同值。当前缺字段/类型行是待实施缺口，不是本轮通过。 |
| TYPE-ARRAY/JDBC→双向关联 | 参数23→一行(23,int4,0,1007)，25→(25,text,0,1009)，1007→(1007,_int4,23,0)，1009→(1009,_text,25,0)，描述26/19/26/26。JDBC探针两参数分别返回单个逗号、内部char18/长度1；这是元数据查询，不证明java.sql.Array已可收发。另以完整SQL `SELECT t.oid,t.typname,t.typelem,t.typcategory FROM pg_catalog.pg_type t WHERE t.oid IN (22,30) ORDER BY t.oid;`，Q参数[]，目标两行(22,int2vector,21,A)/(30,oidvector,26,A)，描述26/19/26/18；其独立OID及vector专用语义沿第3章，不当作1005/1028。 |
| TYPE-CLOSURE→列往返/刷新 | 上述join精确3行，attnum1/2/3，atttypid=t.oid=23/1043/1700，name=int4/varchar/numeric，namespace11，展示integer/character varying(24)/numeric(12,3)；描述21/26/26/19/26/25，长度2/4/4/64/4/-1。用户关系OID按本端关联不硬比较。执行 `ALTER TABLE public.type_contract ALTER COLUMN v TYPE varchar(7);` 及 `ALTER TABLE public.type_contract ALTER COLUMN n TYPE numeric(8,2);`，提交后第二连接重查展示character varying(7)/numeric(8,2)，类型行OID不变。再执行 `ALTER TABLE public.type_contract ALTER COLUMN i TYPE bigint;` 后第一列atttypid=t.oid=20、name=int8、展示bigint，原内建int4行保留。已Parse目录查询的新Bind/Execute须刷新行值，输出形状不变；已启动portal缓存沿公共契约。不能由目录变化推断普通SELECT已保真。 |
| TYPE-FORMAT→显示与typmod | 对上面format_type参数SQL，输入(23,-1)/(1043,28)/(1700,786439)/(1042,NULL)/(1042,-1)/(1007,-1)分别得integer / character varying(24) / numeric(12,3) / character / bpchar / integer[]；NULL用Bind长度-1、其余text，参数OID=[26,23]，均1行text25/长度-1/typmod-1。NULL类型OID应SQL NULL，不变成text名称。格式化不改变列/类型定义，numeric负scale等边界继承第3章，另采参考端精确结果。 |
| TYPE-UNKNOWN/ERROR→未知与缺源 | format_type参数(0,-1)→“-”；`SELECT t.oid FROM pg_catalog.pg_type t WHERE t.oid=4294967295;`，Q参数[]，先在干净参考库确认该OID不存在，随后同一format_type参数(4294967295,-1)→“???”。有效内建(2950,-1)目标uuid，本端缺映射时输出前0A000，不得返回“???”；缺typarray/typlen/typdelim绑定在目录绑定阶段0A000。`SELECT t.typowner,t.typacl FROM pg_catalog.pg_type t WHERE t.oid=23;`，Q参数[]，目标描述26/1034、真实owner与ACL；缺源产品输出前0A000，不能将当前NULL视为正确权限数据。参数0查type参数SQL应零行且描述不变；所有错误保存真实阶段/协议字段，不编造18.6错误文本。 |
| TYPE-DOMAIN→域及用户类型边界 | owner执行 `CREATE DOMAIN public.type_domain AS integer CHECK (VALUE>0);`；若开放后执行 `SELECT t.oid,t.typtype,t.typbasetype,t.typtypmod,t.typndims,t.typnotnull,t.typdefault FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace WHERE n.nspname='public' AND t.typname='type_domain';`，Q参数[]，一行真实域OID/d/23/-1/0/false/NULL，描述26/18/26/23/23/16/25，域OID不得等于23；CHECK使用行为另归第3章。未实现时CREATE在副作用前0A000且无残留域，不造b类型行。没有任何用户类型的隔离库执行同一查询，允许类型正确空集；不能将空集当作已支持域。 |

清理由owner逐条执行，参数[]：`DROP TABLE IF EXISTS public.type_contract;`、`DROP DOMAIN IF EXISTS public.type_domain;`（未开放DOMAIN语法时只检查无残留，不以清理失败替代创建诊断）。保存两端完整版本、角色授权、SQL/Bind、Describe、行值/错误、DDL前后快照与有效用例数；真实psql/JDBC/DataGrip/ORM运行及18.6精确差分仍待取证。本轮未执行任何上述SQL或Rust测试。

#### 索引目录与定义反解

以下 `IDX-*` 均为**后续待新增/待运行**，不是本轮运行结果。统一前提：隔离测试库、UTF8、空闲事务、owner、search_path=public,pg_catalog；第二连接同库、具目录SELECT与函数EXECUTE但无表SELECT，第三连接在另一库建同名表检查隔离。DDL逐条发送并提交；Q参数[]、文本结果，名称/OID按本端身份关联，不比较两端用户OID固定数值。除显式仓库模板外，查询为人工探针。

```sql
CREATE TABLE public.idx_contract (id integer, a integer, b integer, label text,
  CONSTRAINT idx_contract_pk PRIMARY KEY (id));
CREATE INDEX idx_contract_ba ON public.idx_contract (b,a);
CREATE UNIQUE INDEX idx_contract_uq ON public.idx_contract (a);
-- IDX-BASE：输出顺序由名字固定；参数[]。
SELECT c.relname,i.indexrelid,i.indrelid,i.indnatts,i.indnkeyatts,
       i.indisunique,i.indisprimary,i.indnullsnotdistinct,i.indkey,
       i.indisvalid,i.indisready
FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid
WHERE i.indrelid='public.idx_contract'::regclass::oid ORDER BY c.relname;
-- IDX-ORDER：Parse OID=[26]，Bind为本端idx_contract_ba的OID。
SELECT i.indkey,i.indkey[0],i.indkey[1],i.indoption
FROM pg_catalog.pg_index i WHERE i.indexrelid=$1;
-- IDX-DEF：同一OID参数；IDX-UNKNOWN复用此完整SQL。
SELECT pg_catalog.pg_get_indexdef($1),pg_catalog.pg_get_indexdef($1,0,false),
       pg_catalog.pg_get_indexdef($1,1,true),pg_catalog.pg_get_indexdef($1,2,true),
       pg_catalog.pg_get_indexdef($1,99,false),pg_catalog.pg_get_indexdef($1,-1,true);
-- IDX-AUX/STATE：同一OID参数。
SELECT i.indclass,i.indcollation,i.indexprs,i.indpred,i.indisvalid,i.indisready
FROM pg_catalog.pg_index i WHERE i.indexrelid=$1;
-- IDX-EXTRA：同一OID参数。
SELECT i.indisexclusion,i.indimmediate,i.indisclustered,i.indcheckxmin,
       i.indislive,i.indisreplident FROM pg_catalog.pg_index i WHERE i.indexrelid=$1;
```

参数查询在DDL前Parse/Describe statement，OID=[26]，Bind格式[0]、值为本端索引OID十进制文本，分别结果格式[0]/[1]，Describe portal/Execute/Sync。后续修改DDL后新Bind/Execute应读新快照；已启动portal缓存不据此要求中途改行。IDX-DEF无FROM查询是否进入当前目录路径需要单独验证，不能由FROM pg_index测试推断已支持。

| 用例→需求 | 期望结果/真实变更或拒绝阶段 |
| --- | --- |
| IDX-BASE/FLAGS→普通/复合/主键 | 恰3行（ba、pk、uq按名字序），分别列数2/1/1、键数2/1/1、unique=false/true/true、primary=false/true/false、nullsnotdistinct均false、indkey为3 2 / 1 / 2，valid/ready均true（同步完成前提）。描述OID依次19/26/26/21/21/16/16/16/22/16/16，长度64/4/4/2/2/1/1/1/-1/1/1，typmod均-1。indrelid连所属表pg_class.oid，indexrelid连索引pg_class.oid，regclass按各自名称往返一致。第二角色结果相同但 `SELECT * FROM public.idx_contract;` 应权限拒绝；第三库不泄漏。另独立重置后执行 `INSERT INTO public.idx_contract(id,a) VALUES (1,NULL),(2,NULL);` 应成功，随后 `INSERT INTO public.idx_contract(id,a) VALUES (3,7);` 成功，再 `INSERT INTO public.idx_contract(id,a) VALUES (4,7);` 唯一性拒绝；实际SQLSTATE/字段另采，不只验布尔标志。 |
| IDX-ORDER/OPTIONS→索引顺序 | 参数ba时1行向量3 2、两个下标3/2、indoption向量0 0，描述22/21/21/22。另独立夹具 `CREATE INDEX idx_contract_desc ON public.idx_contract (b DESC NULLS LAST,a ASC NULLS FIRST);`，目标indkey=3 2，btree indoption=1 2；若无排序保真来源，DDL副作用前0A000且无残留，不能接受后输出0 0。 |
| IDX-DEF→整定义/逐列 | 参数ba返回1行6列text25/-1；前两列语义等价于 `CREATE INDEX idx_contract_ba ON public.idx_contract USING btree (b, a)`，第三/四列为b/a的合法引用片段，非表中第1/2列id/a。true/false格式保存原始参考文本后比较含义与必要引用；不规定所有标识符必须加引号。第5/6列目标均为空字符串（长度0、非NULL）：REL_18_0的worker在attrsOnly模式只追加匹配列，越界不匹配而返回空buffer，这是源码推导，尚无18.6实测；当前测试断言NULL与此不同，后续实施需同时修正代码和独立测试。输出的普通索引整定义须在隔离克隆夹具（同表列、无原索引名）重建，再查BASE/ORDER核对语义；不在原表上盲执行冲突定义。 |
| IDX-HANDLE→虚拟主索引 | owner另建 `CREATE TABLE public.idx_handle (id integer PRIMARY KEY);`。产品须保存原生model的PKIsHandle、主键列和Indices快照，确实PKIsHandle=true且无显式Primary时，BASE将关系替换为idx_handle，目标恰1个unique/primary=true、indkey=1的索引，连pg_class及本端实际主索引名。不要用PG默认名断言产品当前PRIMARY名，命名规范沿关系身份块；缺模型证据不能标虚拟分支通过。PG端正常主键作为语义对照；不将整数句柄机制冒称PG存储机制。 |
| IDX-AUX/STATE/EXTRA→真实辅助身份 | AUX描述30/30/194/194/16/16；ba的opclass向量长2且每项关联真实pg_opclass、方法/整数类型，collation向量0 0（整数键），表达式/谓词均SQL NULL，valid/ready=true。EXTRA描述六列bool16，普通同步夹具目标false/true/false/false/true/false；当前缺字段应明确0A000，不能补默认值过关。中间态模型以独立模型夹具保存State/BackfillState及真实读写可用状态，不把StatePublic单独当通过证据；线上DDL细节另任务。 |
| IDX-QUOTE→引用完整性 | owner独立执行 `CREATE TABLE public."Idx Mixed" ("a""b" integer);`、`CREATE INDEX "ix""q" ON public."Idx Mixed" ("a""b");`；用完整SQL `SELECT pg_catalog.pg_get_indexdef(i.indexrelid),pg_catalog.pg_get_indexdef(i.indexrelid,1,true) FROM pg_catalog.pg_index i WHERE i.indrelid='public."Idx Mixed"'::regclass::oid;`，参数[]，目标1行两列text25，整定义保留三个名字与双引号转义，逐列片段为 `"a""b"`。改为 `SET search_path=pg_catalog;` 后限定关系查询仍找到对象且定义可正确定位public；清理前RESET。 |
| IDX-LIFE→改名/删除/重建/刷新 | 保存ba OID并prepare上述参数查询。逐条 `ALTER INDEX public.idx_contract_ba RENAME TO idx_contract_ba_new;`、`ALTER TABLE public.idx_contract RENAME COLUMN b TO b_new;`，提交后新Bind/Execute：同一OID、indkey仍3 2，定义更新索引/列名；第二连接同步可见，BASE按新名定位。`DROP INDEX public.idx_contract_ba_new;` 后参数ORDER零行、描述不变，DEF一行NULL整定义；重建 `CREATE INDEX idx_contract_ba_new ON public.idx_contract (b_new,a);` 后新持久ID产生新OID，旧OID不得指向新对象。索引身份重启稳定性沿REL-LIFE另验证，不在本轮运行。 |
| IDX-UNKNOWN/EMPTY→未知/零行 | 独立重置夹具，对ORDER参数0→零行且完整描述，DEF参数0或SQL NULL（Bind长度-1）→一行NULL值；不能因FROM无行而丢失该函数验收。无索引表 `CREATE TABLE public.idx_empty (n integer);`，BASE将关系替换为idx_empty应零行且描述完整。错误参数签名 `SELECT pg_catalog.pg_get_indexdef(0,1,true,0);` 产品绑定/分析拒绝，PG精确诊断另采，不从本地0A000反推PG SQLSTATE。 |
| IDX-SHAPE→表达式/部分/包含/方法/NULL策略 | 每例独立重置idx_contract后逐条输入：`CREATE INDEX idx_expr ON public.idx_contract ((a+1));`、`CREATE INDEX idx_partial ON public.idx_contract (a) WHERE b>0;`、`CREATE INDEX idx_include ON public.idx_contract (a) INCLUDE (b);`、`CREATE INDEX idx_hash ON public.idx_contract USING hash (a);`、`CREATE UNIQUE INDEX idx_nulls ON public.idx_contract (a) NULLS NOT DISTINCT;`。PG参考端分别建立并查BASE/AUX/DEF：表达式indkey=0且indexprs非NULL；部分indpred非NULL；包含列indnatts=2/indnkeyatts=1/indkey=2 3；hash定义保留USING hash；NULL唯一索引标志true且重复NULL插入拒绝。产品缺等价来源/行为时每例DDL副作用前0A000，索引不残留；既有原生表达式、条件、前缀或非等价方法由独立模型/原生夹具注入后反解须在输出成功行前0A000，不生成简化btree定义。拒绝后重查BASE确认无半建对象。 |
| IDX-TEMPLATE/ERROR→客户端需求/坏模型 | 仓库RetrieveIndexColumns.sql保持全文，仅将 `?` 换为wire `$1`，Parse OID=[26]、Bind本端public namespace OID；BASE三索引应展开4行，ba两行k=1/2、column_position=3/2、in_key=true；操作符类/排序规则辅助字段按真实目录关联，不能把当前NULL视为完整支持。模板13列目标OID依次26/20/16/21/21/26/19/19/26/19/19/25/16，typlen为4/8/1/2/2/4/64/64/4/64/64/-1/1（标准name构建），typmod均-1；逐列Describe与text/binary原始值仍须另采，未取证前不声称DataGrip通过。缺model_meta、异常句柄、多主键标记、非法Offset/ID、缺列或未知方法等逐项模型夹具应拒绝且无成功行；被WHERE排除的坏对象也单独取证，不预设provider下推。 |

清理由owner按实际已创建对象执行：普通组 `DROP TABLE IF EXISTS public.idx_contract;`（若重置前曾改表名则使用实际名）、`DROP TABLE IF EXISTS public.idx_handle;`、`DROP TABLE IF EXISTS public.idx_empty;`、`DROP TABLE IF EXISTS public."Idx Mixed";`，所属索引随表清除；逐个复杂形态组重置并核查没有残留索引，不用CASCADE掩盖依赖。关闭prepared/portal、RESET search_path，错误恢复沿第5/7章；保存两端完整版本、角色授权、DDL前后模型/目录快照、SQL与Bind、Describe及行/错误。psql 18后续用 `-E` 实采 `\di public.idx_contract*` 和 `\d public.idx_contract`，真实DataGrip另采版本/驱动/SQL。所有产品、客户端及18.6参考运行仍待完成。

#### 约束目录与定义反解

以下 `CON-*` 全为**后续待新增/待运行**，本轮没有执行SQL。统一前提：隔离库、UTF8、owner、空闲事务、search_path=public,pg_catalog；DDL逐条提交，不据此放开第5章显式事务DDL门禁。第二连接同库具目录SELECT及函数EXECUTE、无表SELECT；第三连接另一库有同名表。Q参数[]、文本结果；用户OID以本端关联比较，不跨端比较固定数值。完整PG18参考端允许下列语句，产品缺能力时保留相应0A000副作用前拒绝证据，不以缺口当成功实现。

```sql
CREATE TABLE public.con_parent (a integer, b integer, code integer,
  CONSTRAINT con_pk PRIMARY KEY (a,b), CONSTRAINT con_uq UNIQUE (code));
CREATE TABLE public.con_child (x integer, y integer, amount integer,
  CONSTRAINT con_fk FOREIGN KEY (y,x) REFERENCES public.con_parent (a,b)
    ON UPDATE RESTRICT ON DELETE CASCADE,
  CONSTRAINT con_ck CHECK (amount > 0));
-- CON-BASE：限定四类，PG18的主键NOT NULL约束另验，不能误计入4行。
SELECT c.oid,c.conname,c.contype,c.conrelid,c.connamespace,c.conindid,
       c.confrelid,c.conkey,c.confkey,c.confupdtype,c.confdeltype
FROM pg_catalog.pg_constraint c
WHERE c.conrelid IN ('public.con_parent'::regclass::oid,
                    'public.con_child'::regclass::oid)
  AND c.contype IN ('p','u','f','c') ORDER BY c.conname;
-- CON-JOIN：Parse OID=[26]，Bind本端con_fk约束OID，不是索引OID。
SELECT c.conrelid,c.confrelid,i.indrelid,c.conindid,i.indexrelid,c.conkey,c.confkey
FROM pg_catalog.pg_constraint c JOIN pg_catalog.pg_index i ON c.conindid=i.indexrelid
WHERE c.oid=$1;
-- CON-DEF/UNKNOWN：Parse OID=[26]，每例换本端约束OID、0或NULL。
SELECT pg_catalog.pg_get_constraintdef($1),
       pg_catalog.pg_get_constraintdef($1,false),pg_catalog.pg_get_constraintdef($1,true);
-- CON-STATE：同一OID参数，完整状态，当前缺字段不能宣称已返回false。
SELECT c.condeferrable,c.condeferred,c.convalidated,c.conenforced
FROM pg_catalog.pg_constraint c WHERE c.oid=$1;
-- CON-CHECK：参数为con_ck OID。
SELECT c.conkey,c.conbin,pg_catalog.pg_get_constraintdef(c.oid)
FROM pg_catalog.pg_constraint c WHERE c.oid=$1;
-- CON-EXTRA：参数为con_fk OID。
SELECT c.confmatchtype,c.conislocal,c.coninhcount,c.conparentid,c.contypid,
       c.conpfeqop,c.conppeqop,c.conffeqop,c.confdelsetcols,c.conperiod,
       c.connoinherit,c.conexclop FROM pg_catalog.pg_constraint c WHERE c.oid=$1;
```

参数查询分别在DDL前Parse/Describe statement，OID=[26]；Bind格式[0]，值为本端OID十进制文本或NULL长度−1，结果格式[0]/[1]分别运行，Describe portal/Execute/Sync。无FROM的DEF须独立验证分派与一行结果，不能以FROM零行的测试替代。每次DDL提交后新Bind/Execute取新快照；已启动portal的缓存边界沿公共块。

| 用例→字段/需求 | 预期行、真实变更与拒绝证据 |
| --- | --- |
| CON-BASE/ORDER→四类与顺序 | 恰4行，按名字ck/fk/pk/uq；contype=c/f/p/u，conkey分别{3}/{2,1}/{1,2}/{3}，仅FK的confkey={1,2}，其他为NULL。FK两侧数组下标1/2按y→a、x→b对应；描述依序26/19/18/26/26/26/26/1005/1005/18/18，长度4/64/1/4/4/4/4/−1/−1/1/1，typmod均−1。ck的conindid=0、非FK的confrelid=0；FK动作r/c，非FK动作零字节。当前有公开CHECK会拒绝，若产品建表前0A000则确认con_child无残留；p/u/f子集可用不带CHECK的同结构子夹具单独验证，不能称四类目标通过。 |
| CON-JOIN/IDENTITY→身份闭环 | FK参数恰1行，i.indrelid=c.confrelid=父表OID，c.conrelid=子表OID，c.conindid=i.indexrelid=con_pk的支持索引OID；描述26/26/26/26/26/1005/1005。另执行 `CREATE UNIQUE INDEX con_standalone ON public.con_parent (b,a);`，BASE仍4行，不能多出u；同名约束可存在另一表，执行 `CREATE TABLE public.con_other (n integer CONSTRAINT con_ck CHECK (n>0));`，其约束OID独立，带conrelid定位不混淆。缺真实独立索引/约束来源时登记缺口，不按现有混同行为降低目标。 |
| CON-DEF/CHECK→可重建定义 | DEF每OID一行3列text25/−1，p/u/c/f分别为语义等价于PRIMARY KEY (a,b)、UNIQUE (code)、CHECK (amount>0)、FOREIGN KEY (y,x) REFERENCES public.con_parent(a,b) ON UPDATE RESTRICT ON DELETE CASCADE的片段；引用可随有效path省略public，pretty仅格式差异。CHECK查询1行描述1005/194/25、conkey={3}、conbin非NULL且为真实树；先独立逐条建立克隆夹具 `CREATE TABLE public.con_clone_parent (a integer,b integer,code integer);`、`CREATE TABLE public.con_clone (x integer,y integer,amount integer);`，以DEF中p/u片段分别拼接 `ALTER TABLE public.con_clone_parent ADD CONSTRAINT con_clone_pk ` / `ALTER TABLE public.con_clone_parent ADD CONSTRAINT con_clone_uq ` 并执行。以f片段拼接 `ALTER TABLE public.con_clone ADD CONSTRAINT con_clone_fk `（仍引用原con_parent），再用 `ALTER TABLE public.con_clone ADD CONSTRAINT con_clone_ck ` 拼接已取得CHECK片段，实际执行后重查；逐条 `INSERT INTO public.con_clone(amount) VALUES (0);` 须拒绝、`INSERT INTO public.con_clone(amount) VALUES (NULL);` 与 `INSERT INTO public.con_clone(amount) VALUES (1);` 须成功（FK列均NULL）。用BASE将两表替换为con_clone_parent/con_clone，目标p/u/f/c共4行、FK仍关联原父表。不能只比较字符串。缺转换在成功行前0A000；不返回简化或NULL定义。 |
| CON-UNKNOWN/EMPTY→缺对象与签名 | DEF绑定0、确定未分配的OID或NULL均1行3列NULL；对应 `SELECT c.oid,c.conkey FROM pg_catalog.pg_constraint c WHERE c.oid=$1;` 以未知OID绑定应0行，描述26/1005完整。`SELECT pg_catalog.pg_get_constraintdef(0,false,0);` 产品分析/绑定拒绝且无行，PG精确SQLSTATE与位置另采。现有CHECK/坏模型导致未知OID的fallback也报错需记录，不掩盖为正常缺对象行为。 |
| CON-STATE/EXTRA→模式与真实验证 | 基础夹具STATE每约束false/false/true/true、描述四列16；只有即时执行且建约束已验证事实具备才可通过。EXTRA对FK为s/true/0/0/0、三个真实等号操作符数组（每个长度2且关联pg_operator）、NULL/false/true/NULL，描述18/16/21/26/26/1028/1028/1028/1005/16/16/1028。另独立夹具 `CREATE TABLE public.con_defer (v integer CONSTRAINT con_du UNIQUE DEFERRABLE INITIALLY DEFERRED);`，STATE目标true/true/true/true；PG参考 `BEGIN; INSERT INTO public.con_defer VALUES (1),(1); COMMIT;` 应在COMMIT拒绝。产品无延迟来源则DDL前0A000且无对象，不填false后允许创建。 |
| CON-VALIDATE→未验证/强制执行区分 | 独立建 `CREATE TABLE public.con_validate (n integer);`，`INSERT INTO public.con_validate VALUES (-1);`，`ALTER TABLE public.con_validate ADD CONSTRAINT con_cv CHECK (n>0) NOT VALID;`，STATE目标false/false/false/true。新插入−2拒绝；`ALTER TABLE public.con_validate VALIDATE CONSTRAINT con_cv;` 因既有−1拒绝且状态仍false。`DELETE FROM public.con_validate WHERE n<0;` 后重试VALIDATE，目标validated=true，反解同步去除NOT VALID。每步保存真实行与状态；产品无验证状态时添加前0A000且既有−1保留、无约束残留。不得把CHECK.Enforced=false当NOT VALID。 |
| CON-ACTION→执行与定义一致 | 基础夹具依次 `INSERT INTO public.con_parent VALUES (10,20,1);`、`INSERT INTO public.con_child VALUES (20,10,1);`，`UPDATE public.con_parent SET a=11 WHERE a=10 AND b=20;` 因RESTRICT拒绝、两表值不变；`DELETE FROM public.con_parent WHERE a=10 AND b=20;` 后父子均无对应行。用完整查询 `SELECT x,y,amount FROM public.con_child ORDER BY y,x;` 检查。NO ACTION、SET NULL、SET DEFAULT与MATCH FULL/部分列删除动作另以明确动作DDL逐个窄化，未有执行来源不能只靠码值宣布支持；第5章负责写入/提交/失败恢复，不在本块实现。 |
| CON-LIFE/QUOTE→元数据更新 | 保存FK OID并prepare JOIN/DEF。逐条 `ALTER TABLE public.con_child RENAME CONSTRAINT con_fk TO con_fk_new;`、`ALTER TABLE public.con_child RENAME COLUMN y TO y_new;`、`ALTER TABLE public.con_parent RENAME COLUMN a TO a_new;`，新Bind/Execute的OID及{2,1}/{1,2}不变、名称/定义同步，第二连接可见；`ALTER TABLE public.con_child DROP CONSTRAINT con_fk_new;` 后旧OID JOIN零行、DEF一行NULL，重加 `ALTER TABLE public.con_child ADD CONSTRAINT con_fk_new FOREIGN KEY (y_new,x) REFERENCES public.con_parent(a_new,b) ON UPDATE RESTRICT ON DELETE CASCADE;` 得新OID。引号独立夹具 `CREATE TABLE public."Con Mixed" ("a""b" integer, CONSTRAINT "ck""q" CHECK ("a""b">0));`，以BASE将关系替换成该表、类型过滤仅c，目标1行，DEF保留列名转义；SET search_path=pg_catalog后仍能限定定位/重建，RESET清理。 |
| CON-ACL/DB→权限与隔离 | 第二角色对BASE/DEF结果与owner一致，完整 `SELECT * FROM public.con_child;` 权限拒绝；具目录与函数授权不代表表SELECT。第三库同名关系不出现在本库BASE，OID关联不误跨库。没有角色/跨库当前运行证据，不能从provider未过滤直接推导授权正确。 |
| CON-TEMPLATE/ERROR→客户端及坏模型 | 仓库1869280154.sql全文保留，仅将第1/2个?分别换$1/$2，Parse OID=[26,26]、Bind均为本端public namespace OID。基础四类行含p/u/f/c，排除操作符原字段NULL、展开{}；CHECK表达式必须真实反解，xmin状态提示缺证按上表记录，不能伪造事务ID。18列目标OID依序26/18/20/20/19/18/1005/26/26/16/16/18/18/16/25/1005/1007/1015（最后为varchar[]，数组转换沿第3章另验）；typmod均−1。无完整字段/codec时明确拒绝，不称DataGrip通过。缺模型、FK无父表/父唯一索引、列数不等、坏ID/Offset、非当前库引用、未公开父模型、未知动作逐项独立模型夹具，预期输出成功行前拒绝；WHERE排除坏对象另存证据，不假设下推。 |

清理按实际建立顺序反向执行 `DROP TABLE IF EXISTS public.con_clone;`、`DROP TABLE IF EXISTS public.con_clone_parent;`、`DROP TABLE IF EXISTS public.con_child;`、`DROP TABLE IF EXISTS public.con_parent;`、`DROP TABLE IF EXISTS public.con_other;`、`DROP TABLE IF EXISTS public.con_defer;`、`DROP TABLE IF EXISTS public.con_validate;`、`DROP TABLE IF EXISTS public."Con Mixed";`，其他克隆以实际名字清除；失败事务先按第5章ROLLBACK，不用CASCADE隐藏依赖。关闭prepared/portal，RESET search_path。后续采集两端完整构建/角色/授权、模型及目录前后快照、SQL/Bind/Describe/行/错误与有效场景数；psql18用-E实采 `\d public.con_child`，DataGrip保存版本/驱动/真实SQL。产品功能、18.6及客户端结果均未运行。

#### 默认表达式目录契约

以下均为待新增/待运行人工契约探针，现有测试仅作意图入口。前提：UTF8、标准字符串开启、当前库test、path=public,pg_catalog，owner拥有建表/改表权限；Q参数[]、文本结果，扩展路径以同SQL执行Parse/Describe/Bind/Execute/Sync。DDL逐条执行，若产品缺能力须在副作用前0A000并确认无残留，记录目标未满足，不能把拒绝当完整成功。

```sql
CREATE TABLE public.def_probe (plain integer, n integer DEFAULT NULL,
  literal text DEFAULT 'NULL', qty integer DEFAULT 7,
  label text DEFAULT 'O''Reilly', flag boolean DEFAULT true);
SELECT a.attnum,a.attname,a.atthasdef,d.oid,d.adrelid,d.adnum,d.adbin,
       pg_catalog.pg_get_expr(d.adbin,d.adrelid) AS definition,
       pg_catalog.pg_get_expr(d.adbin,d.adrelid,true) AS pretty
FROM pg_catalog.pg_attribute a LEFT JOIN pg_catalog.pg_attrdef d
  ON d.adrelid=a.attrelid AND d.adnum=a.attnum
WHERE a.attrelid='public.def_probe'::regclass::oid AND a.attnum>0
ORDER BY a.attnum;
```

将上式记为 BASE；列描述目标为21/19/16/26/26/21/194/25/25（attname目标name沿列块，当前text适配），typmod均−1。每个输出均由上表来源映射；LEFT JOIN缺默认时d.*与两个反解为SQL NULL。

| 验收ID | 完整输入/参数与期望、关联事实 |
| --- | --- |
| DEF-BASE | BASE返回6列对应的6行；plain无默认，n按PG参考构建规范化结果记录，禁止未经实测固定其目录行数；literal/qty/label/flag必须有默认行，adnum分别3/4/5/6，adrelid为真实表OID，oid非零且互异，adbin非NULL；反解分别语义为文本NULL、整数7、文本O'Reilly、bool true，pretty语义相同。`INSERT INTO public.def_probe DEFAULT VALUES; SELECT plain,n,literal,qty,label,flag FROM public.def_probe;` 1行：NULL/NULL/文本NULL/7/O'Reilly/true。反解后在同类型独立克隆列上执行ALTER SET DEFAULT并插入，验证值相等，不能只比较字符串。 |
| DEF-ARG | `SELECT pg_catalog.pg_get_expr(d.adbin,0,false) FROM pg_catalog.pg_attrdef d WHERE d.adrelid='public.def_probe'::regclass::oid AND d.adnum=4;` 1行text25且等价7，无Vars不能错误要求owner相等。`SELECT pg_catalog.pg_get_expr(NULL,0);` 1行NULL/text25；`SELECT pg_catalog.pg_get_expr('fake',0);` 产品在解析树/执行阶段拒绝且无行，精确PG SQLSTATE另采；`SELECT pg_catalog.pg_get_expr(NULL,0,false,0);` 分析/绑定阶段拒绝错误签名。 |
| DEF-META | 先扩展Parse BASE，执行 `ALTER TABLE public.def_probe ALTER COLUMN qty SET DEFAULT 9;` 再首次Execute，应见9；重新绑定执行不可复用旧默认。随后 `ALTER TABLE public.def_probe ALTER COLUMN label DROP DEFAULT;`、`ALTER TABLE public.def_probe RENAME COLUMN qty TO quantity;`，BASE仍6行，label的d.*为NULL，quantity保留属性关联且定义9。`SELECT d.oid,d.adrelid,d.adnum,d.adbin FROM pg_catalog.pg_attrdef d WHERE d.adrelid=0;` 0行，描述26/26/21/194完整；drop/recreate表后旧表OID不得关联新默认。每步保存真实模型ID/列ID/offset；其他连接重读按第5章提交边界观察，已启动portal按快照契约处理。 |
| DEF-GENERATED | 独立执行 `CREATE TABLE public.def_gen (x integer, y integer GENERATED ALWAYS AS (x+1) STORED);`，将BASE中的表名替换为def_gen；2行，x无定义，y有默认目录行且adnum=2，attgenerated另查 `SELECT attgenerated FROM pg_catalog.pg_attribute WHERE attrelid='public.def_gen'::regclass::oid AND attname='y';` 得s/char18。反解语义x+1；`INSERT INTO public.def_gen(x) VALUES (2); SELECT x,y FROM public.def_gen;` 得2/3。VIRTUAL用同结构新表def_virtual将STORED改VIRTUAL，目标标记v，缺真实语义则DDL前拒绝。`SELECT column_name,column_default,is_generated,generation_expression FROM information_schema.columns WHERE table_schema='public' AND table_name='def_gen' ORDER BY ordinal_position;` 2行，x为NULL/NEVER/NULL，y为NULL/ALWAYS/等价x+1，四列域类型按后续视图契约取证。 |
| DEF-TIME/REJECT | 独立执行 `CREATE TABLE public.def_time (t timestamp with time zone DEFAULT CURRENT_TIMESTAMP);`，BASE替换表名仅1行，反解保留运行时函数，内省前后不得写表；在同一事务两次默认插入须符合第5章事务时间契约，不能在DDL时折叠时间。原生MySQL连接建立 `CREATE TABLE def_native (x INT, y INT GENERATED ALWAYS AS (IFNULL(x,0)) STORED);` 后PG查询BASE替换为def_native，应在行结果前0A000，不能返回原生文本/假NULL。若建立本身不支持，记录未建立及原因，不冒称已验证读取拒绝。 |
| DEF-ACCESS | owner运行BASE；另角色def_reader无表授权时，按PG参考构建核对pg_catalog可读定义，而information_schema上述查询（表名替换def_probe）应0行。`GRANT SELECT ON public.def_probe TO def_reader;` 后视图列可见但仍无写权限；须用真实认证连接，禁止只改会话字符串模拟。另数据库同名表不能入当前库BASE。完整角色创建/认证与权限采集沿后续权限块，本轮未运行。 |

DEF-BASE克隆的完整准备为 `CREATE TABLE public.def_clone (v text);`，取label反解结果作为唯一SQL表达式参数拼接 `ALTER TABLE public.def_clone ALTER COLUMN v SET DEFAULT `，执行后 `INSERT INTO public.def_clone DEFAULT VALUES; SELECT v FROM public.def_clone;` 应为O'Reilly，随后 `DROP TABLE public.def_clone;`；其他常量用同一流程按原列类型分别建克隆，不把返回文本交给未验证的通用执行入口。DEF-TIME的完整事务探针为 `BEGIN; INSERT INTO public.def_time DEFAULT VALUES; INSERT INTO public.def_time DEFAULT VALUES; SELECT t FROM public.def_time; ROLLBACK;`，两行非NULL且相等，反解查询本身不插入行；墙钟推进/跨事务时间另采。DEF-GENERATED另执行 `UPDATE public.def_gen SET x=4; SELECT x,y FROM public.def_gen;` 应为4/5，确保目录表达式与实际重新计算一致。

清理按实际建立对象逐条执行 `DROP TABLE public.def_probe;`、`DROP TABLE public.def_gen;`、`DROP TABLE public.def_virtual;`、`DROP TABLE public.def_time;`，原生夹具在所属连接执行 `DROP TABLE def_native;`；权限夹具清理角色由其任务负责。psql后续以固定18.x客户端的 `psql -E` 执行 `\d+ public.def_probe` 保存完整SQL/版本/输出，不能把上述手写BASE称客户端验收通过。

#### 视图定义内省契约

以下是待新增/待运行的人工契约探针，非客户端抓包或测试通过。前提：UTF8、标准字符串开启、当前库test、path=public,pg_catalog，真实owner有建表/视图/改名权限，Q参数[]、文本结果；扩展同SQL经Parse/Describe/Bind/Execute/Sync，BASE改为 `WHERE c.oid=$1` 时唯一参数是所取视图OID、类型26，分别文本十进制与4字节网络序。缺DDL能力须在副作用前0A000、检查无残留并记录目标未满足。

```sql
CREATE TABLE public.view_base (id integer, label text);
INSERT INTO public.view_base VALUES (1,'AS WITH LOCAL CHECK OPTION');
CREATE VIEW public."ViewAS" ("ID", "Label") AS
  /* AS ignored */ SELECT id AS "ID", label AS "Label"
  FROM public.view_base WHERE id > 0 WITH LOCAL CHECK OPTION;
SELECT c.oid,c.relnamespace,c.relname,c.relkind,
       pg_catalog.pg_get_viewdef(c.oid) AS definition,
       pg_catalog.pg_get_viewdef(c.oid,false) AS plain,
       pg_catalog.pg_get_viewdef(c.oid,true) AS pretty
FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid
WHERE n.nspname='public' AND c.relname='ViewAS';
```

上式查询记BASE；一行，描述26/26/19/18/25/25/25（当前relname文本适配沿关系块），所有定义非NULL且语义相等；OID/namespace与真实模型一致，relkind=v，结果列仍ID/Label。输出不含CREATE外壳或尾CHECK OPTION，不要求保留注释。以下每项均关联上表来源，不能仅比较字符串命中。

| 验收ID | 完整输入/参数、期望与元数据变化 |
| --- | --- |
| VIEW-BASE | 执行BASE，扩展参数版返回同一行；传NULL或已证实不存在的OID参数返回0行且Describe完整。取plain作为单一查询定义，构造 `CREATE VIEW public.view_clone AS ` 后追加返回文本，执行 `SELECT * FROM public."ViewAS"; SELECT * FROM public.view_clone;` 各为1行(1,字面量AS WITH LOCAL CHECK OPTION)，列名/类型一致。使用AST或解析后的语义检查，不以分号、空白完全相等验收。 |
| VIEW-LEX | 逐条执行 `CREATE VIEW public."as" AS SELECT 'AS WITH CASCADED CHECK OPTION' AS "aS";`、`CREATE VIEW public.view_cascade AS SELECT id FROM public.view_base WHERE id>0 WITH CASCADED CHECK OPTION;`、`CREATE VIEW public.view_default_check AS SELECT id FROM public.view_base WHERE id>0 WITH CHECK OPTION;`；BASE的relname分别换as/view_cascade/view_default_check，每项1行，第一项重建执行得完整字面量及大小写aS，后两项定义只保留SELECT条件。存储级补测CREATE头列名含AS、注释在AS前后、尾分号、裸SELECT/集合操作、多语句及非SELECT；合法文本不误切，损坏模型在执行行前拒绝，不把存储损坏等同未知OID。 |
| VIEW-ARG | `SELECT pg_catalog.pg_get_viewdef('public.view_base'::regclass::oid),pg_catalog.pg_get_viewdef(0::oid),pg_catalog.pg_get_viewdef(NULL::oid),pg_catalog.pg_get_viewdef('public."ViewAS"'::regclass::oid,NULL::boolean);` 一行四个NULL/text25，分别是非视图/未知/空OID/空pretty。`SELECT pg_catalog.pg_get_viewdef('public."ViewAS"'::regclass::oid,40);` 一行text25且等价BASE，换行目标另采；`SELECT pg_catalog.pg_get_viewdef('public."ViewAS"'::text,true);` 旧名称签名同定义。未支持签名须绑定阶段0A000并记为缺口；`SELECT pg_catalog.pg_get_viewdef(0::oid,true,false);` 错误数量绑定拒绝，精确PG SQLSTATE未采。 |
| VIEW-META | 先Parse BASE，执行 `CREATE OR REPLACE VIEW public."ViewAS" ("ID","Label") AS SELECT id,label FROM public.view_base WHERE id>1 WITH LOCAL CHECK OPTION;` 后首次Execute见新条件；重新执行视图查询0行。`ALTER VIEW public."ViewAS" RENAME TO "RenamedAS";` 后用旧OID参数查询仍一行，relname改为RenamedAS且OID不变；旧名称BASE变0行。`ALTER TABLE public.view_base RENAME COLUMN label TO renamed_label;` 后定义引用新基列而视图列Label不变，执行不失效。删建视图后旧OID查函数NULL、新对象身份沿关系块；其他连接按第5章提交可见性重读，portal不任意混合快照。 |
| VIEW-PATH | `CREATE SCHEMA view_shadow; CREATE TABLE view_shadow.view_base (id integer,label text); INSERT INTO view_shadow.view_base VALUES (99,'shadow'); SET search_path TO view_shadow,public,pg_catalog;` 在META前取ViewAS OID并以参数版BASE重读，重建定义必须限定真实public基表，执行仍得原(1,原label)，不能绑定shadow；改path不改变视图OID及定义依赖。缺用户schema/绑定能力须DDL前拒绝、记录未验证；不把public-only样例当遮蔽通过。 |
| VIEW-REJECT | 在原生连接创建 `CREATE VIEW view_native AS SELECT IFNULL(label,'x') AS label FROM view_base;`，PG按真实OID调用定义函数。若无法证明IFNULL和引用转换等价，执行行前0A000且无部分行/写入；不得直接返回原生库限定或反引号SQL。无法建立夹具则记录原因。独立存储夹具把SelectStmt设为DROP或两条SELECT应拒绝；这些夹具待新增，不是当前可直接运行SQL。 |
| VIEW-ACCESS | owner与真实低权角色分别执行BASE；按参考构建核对有目录读权限但无基表SELECT时定义仍可内省，`SELECT * FROM public."ViewAS";` 的授权结果单独验证。撤销相关目录读权限不得旁路；information_schema.views/pg_views完整投影留对应块，不能以owner固定NULL冒充来源。另一数据库同名视图须隔离，凭本库OID不得读出其他库定义。角色创建/认证与精确错误沿权限块取证。 |

PATH、META与ACCESS使用独立重建夹具，避免改名污染BASE；VIEW-REJECT在META前创建。每次重建保存真实模型ID、ViewInfo.SelectStmt/Cols/CheckOption和目录输出。清理按实际对象执行 `SET search_path TO public,pg_catalog; DROP VIEW public.view_clone; DROP VIEW public."ViewAS";`（META后换RenamedAS）、`DROP VIEW public."as"; DROP VIEW public.view_cascade; DROP VIEW public.view_default_check; DROP TABLE public.view_base; DROP TABLE view_shadow.view_base; DROP SCHEMA view_shadow;`；原生view_native在删基表前由所属连接删除。后续固定psql 18.x以 `psql -E` 执行 `\d+ public."ViewAS"` 保存版本/完整查询/结果；DataGrip存档SQL与实际UI另采，均未在本轮运行。

#### 序列身份与依赖契约

以下 `SEQDEP-*` 均为**待新增/待运行**的后续验收输入，非本轮执行结果。统一 UTF8、autocommit I、owner 角色、独立空 schema `seqdep`，无并发分配或重置；Q 用参数 []、文本结果，扩展链用 Parse/Bind/Describe/Execute/Sync、注明参数 OID及文本格式。PG fixture 为：

```sql
CREATE SCHEMA seqdep;
CREATE TABLE seqdep.t (id bigint, plain bigint);
CREATE SEQUENCE seqdep.s AS bigint START WITH 7 INCREMENT BY 3
  MINVALUE 1 MAXVALUE 100 CACHE 1 NO CYCLE OWNED BY seqdep.t.id;
CREATE TABLE seqdep.ia (id integer GENERATED ALWAYS AS IDENTITY);
CREATE TABLE seqdep.ib (id bigint GENERATED BY DEFAULT AS IDENTITY);
```

1. **SEQDEP-01 参数/零行**，Q：`SELECT seqrelid,seqtypid,seqstart,seqincrement,seqmin,seqmax,seqcache,seqcycle FROM pg_catalog.pg_sequence WHERE seqrelid='seqdep.s'::regclass::oid`；恰一行 `(S,20,7,3,1,100,1,false)`，Describe为26/26/20/20/20/20/20/16。S 是真实 pg_class.oid，不填固定样本OID。扩展把条件替为 `seqrelid=$1`，参数 `[S:oid/26]`；再绑 `[NULL:oid/26]`、未分配OID，零行描述不变。ia 所关联序列 seqtypid=23、ib=20，OID须与各自 pg_class 一致；原生 AUTO_INCREMENT 夹具沿已有独立测试确认 relkind=r、无伪序列行。
2. **SEQDEP-02 依赖端点**，Q：`SELECT classid,objid,objsubid,refclassid,refobjid,refobjsubid,deptype FROM pg_catalog.pg_depend WHERE objid='seqdep.s'::regclass::oid ORDER BY refclassid,refobjid,refobjsubid`；至少核对 `(1259,S,0,2615,N,0,n)` 与 `(1259,S,0,1259,T,1,a)`，描述26/26/23/26/26/23/18；不以这两行排除PG其他真实依赖。N/T从pg_namespace/pg_class取，不靠数值巧合关联。重复查询加两端classid守卫关联表与序列，不能把命名空间n边当列拥有。
3. **SEQDEP-03 identity/普通列**，Q：`SELECT attname,attidentity,atthasdef FROM pg_catalog.pg_attribute WHERE attrelid='seqdep.ia'::regclass::oid AND attnum>0 AND NOT attisdropped`；id 的模式a、atthasdef=false，ib对应d；类型沿列目录块（attname=name/19，模式18，布尔16）。`SELECT pg_catalog.pg_get_serial_sequence('seqdep.ia','id')` 返回text/25，解析该名称取得真实S_i，其列拥有边为 `(1259,S_i,0,1259,T_i,1,i)`；ib同理。`SELECT pg_catalog.pg_get_serial_sequence('seqdep.t','plain')` 返回NULL，id返回seqdep.s的可解析名称。函数扩展SQL `SELECT pg_catalog.pg_get_serial_sequence($1,$2)`，参数 `[$1='seqdep.t':text/25,$2='id':text/25]`；普通默认引用不代替拥有关系。当前缺来源时先能力拒绝，不期待现状成功。
4. **SEQDEP-04 无分配副作用**，同连接先 `SELECT nextval('seqdep.s'::regclass)` 得7，再连续两次运行01/02及 `SELECT pg_catalog.pg_get_serial_sequence('seqdep.t','id')`，含扩展Parse/Describe/Execute及NULL零行探针；`SELECT currval('seqdep.s'::regclass)` 仍7，下一次nextval须10。另一条新连接在未nextval前做同样内省，currval仍未初始化（精确诊断待参考采集）。不能把读取SequenceInfo.Start等同当前值，内省内部禁调用nextval/setval或缓存分配。
5. **SEQDEP-05 真实变更**，保存S/T后执行 `ALTER SEQUENCE seqdep.s OWNED BY NONE`，02的a边消失、n边保留、函数对t.id返回NULL；重新OWNED BY恢复。`ALTER SEQUENCE seqdep.s INCREMENT BY 4 CACHE 2` 后01参数更新；重命名表/列/序列须保留各真实OID及列关联而函数输出新名称。新Execute读取提交后的快照，已有portal行为沿首块；DROP再CREATE同名序列须换身份，旧OID不串入新边。identity列删除须同步移除内部序列及依赖；直接DROP内部序列拒绝，a依赖序列可单独删除。清理 `DROP SCHEMA seqdep CASCADE`，不得留下悬空边。
6. **SEQDEP-06 安全/拒绝**，owner 创建 `seqdep_reader` 并仅授予schema USAGE，撤销序列全部权限，切换该角色：01/02的真实目录参数/依赖仍可查，但nextval拒绝；不由目录读取推断分配授权。另连接其他数据库不出现这些对象，search_path遮蔽不影响限定pg_catalog探针。`SELECT missing_field FROM pg_catalog.pg_depend`、`SELECT unsupported_dependency(objid) FROM pg_catalog.pg_depend`、`DELETE FROM pg_catalog.pg_depend` 在目录绑定/能力门禁拒绝，目标0A000；真实权限错误和DROP依赖错误的精确SQLSTATE/阶段须参考采集，不借用0A000。目录查询任何失败不得分配值，后续恢复沿第5章。

01–06需逐列登记源模型ID、关联键、描述、文本/二进制结果与观察记录；已有手写wire测试只覆盖部分现状，不代替上述目标。psql `-E` 下的 `\ds seqdep.*`、`\d seqdep.ia` 只作为待采集客户端入口，驱动/UI结果尚未验证。

#### 函数目录与注册边界

以下 `PROC-01`–`PROC-04` 均为**待新增/待运行**的后续验收，当前独立测试仅提供上述局部意图。公共前提：当前数据库、具备目录SELECT的角色、固定元数据代际；Q与Parse/Describe/Bind/Execute结果及类型一致，空集不丢描述。参数均显式给PG类型，不用MySQL类型代替。

```sql
-- PROC-01：静态适配器基线；不假定这些对象OID等于参考PG对象OID。
SELECT oid,proname,pronamespace,proargtypes,prorettype,proacl
FROM pg_catalog.pg_proc WHERE pronamespace=11 ORDER BY oid;
-- 零行也保留26/19/26/30/26/1034的目标描述。
SELECT oid,proname,pronamespace,proargtypes,prorettype,proacl
FROM pg_catalog.pg_proc WHERE pronamespace=0;
-- PROC-01补充字段：任一未绑定字段都应明确拒绝，不返回伪造值。
SELECT proargnames,proargmodes,proallargtypes,proargdefaults,pronargs,
       pronargdefaults,provariadic,prokind,proretset,provolatile,proisstrict,
       prosecdef,proleakproof,proparallel,prolang,prosrc,proconfig,
       proowner,procost,prorows,xmin
FROM pg_catalog.pg_proc WHERE pronamespace=11 ORDER BY oid;
-- PROC-02：三种反解共享对象身份；扩展协议$1参数OID=26。
SELECT pg_catalog.pg_get_function_arguments(oid),
       pg_catalog.pg_get_function_result(oid),
       pg_catalog.pg_get_function_sqlbody(oid)
FROM pg_catalog.pg_proc WHERE oid=$1;
SELECT pg_catalog.pg_get_function_result(NULL::oid) FROM pg_catalog.pg_language;
-- 绑定/能力门禁必须拒绝未知名，即使过滤结果为空。
SELECT unsupported_user_function(oid) FROM pg_catalog.pg_proc WHERE pronamespace=0;
```

| 编号 | 期望及拒绝阶段；事实变化/权限前提 |
| --- | --- |
| PROC-01 | 当前适配器基线三行分别16000/16001/16002，三个名称、namespace11、输入向量26、返回25、ACL NULL；目标proname为name/19，当前text/25差异应被检出。另逐列查询上表其余字段，核对完整类型、数组NULL、参数数目与关联键；尚未绑定字段须绑定期0A000，不能以编译通过取代字段证据。内建声明不随无关建表变化。 |
| PROC-02 | $1=16000/16001/16002时各一行，三个结果为“function_oid oid”/“text”/NULL，类型25/25/25；NULL参数过滤无行仍保留描述；直接NULL反解返回NULL。$1=0、4294967295以及错误实参数量沿当前适配器拒绝策略，记录分析/执行实际阶段和SQLSTATE，不猜PG错误。未知名在绑定阶段0A000，随后SELECT 1恢复。保留现有测试的语言JOIN与排除internal查询空集意图。 |
| PROC-03 | 隔离参考库中由管理员建立 `CREATE SCHEMA cat_proc12; CREATE FUNCTION cat_proc12.f(integer) RETURNS integer LANGUAGE SQL AS 'SELECT $1'; CREATE FUNCTION cat_proc12.f(bigint) RETURNS bigint LANGUAGE SQL AS 'SELECT $1';`；按namespace查询必须两行不同稳定oid，proargtypes为23/20、prorettype为23/20。普通 `SELECT cat_proc12.f(1::integer),cat_proc12.f(1::bigint);` 得1/1且描述23/20。执行 `DROP FUNCTION cat_proc12.f(integer);` 后只余bigint行，再执行同一integer建函数语句不得错误复用旧对象引用；Parse后DDL、跨连接与search_path遮蔽沿第2/5章。当前用户函数DDL未开放时在DDL能力门禁0A000，不允许声称此用例通过；后续清理 `DROP SCHEMA cat_proc12 CASCADE;`。 |
| PROC-04 | 在PROC-03夹具存在且真实角色/ACL开放后，由管理员 `CREATE ROLE cat_proc12_reader; GRANT USAGE ON SCHEMA cat_proc12 TO cat_proc12_reader; REVOKE EXECUTE ON FUNCTION cat_proc12.f(bigint) FROM PUBLIC;`；该角色目录行仍可枚举，但普通调用无EXECUTE应在授权阶段拒绝。管理员 `GRANT EXECUTE ON FUNCTION cat_proc12.f(bigint) TO cat_proc12_reader;` 后可调用，目录ACL同步；NULL默认ACL与显式ACL分开。查询 `SELECT pg_catalog.has_function_privilege('cat_proc12_reader','cat_proc12.f(bigint)','EXECUTE');` 前后false/true；`SELECT pg_catalog.pg_function_is_visible(oid) FROM pg_catalog.pg_proc WHERE pronamespace=$1;` 的$1取真实schema oid，随有效路径及同签名遮蔽变化，不随EXECUTE权限直接变化。无角色/ACL来源时能力门禁拒绝该流程，不伪造布尔值；清理schema后 `DROP ROLE cat_proc12_reader;`。 |

真实客户端验收另采集 `psql -E` 执行 `\df pg_catalog.pg_get_function_*` 的版本、SQL及描述，并与人工探针分开；JDBC/DataGrip/ORM的原始查询来源尚缺，不从仓库测试注释推定官方生成行为。

#### 对象注释与描述契约

以下COM-*均为**待新增/待运行**，不修改现有测试。前提：PG会话、UTF-8、autocommit、search_path=public、standard_conforming_strings=on，owner可建表与COMMENT、目录可读。维护库owner先执行 `CREATE DATABASE comment_db; CREATE DATABASE comment_other;`，连接comment_db；各COM用例从独立干净夹具执行，使用专用数据库comment_db和comment_other，初始无同名对象；准备 `CREATE TABLE public.comment_contract (a integer, b text);`。无参数SQL参数=[]、Q文本结果；参数查询以Parse/Bind/Describe/Execute/Sync各跑text/binary，用户OID由真实目录查询取得。未实现能力按上节0A000门禁验收，不跳过后声称通过。

```sql
-- COM-ROWS / KEY：owner准备
COMMENT ON TABLE public.comment_contract IS '表 é ''引号''';
COMMENT ON COLUMN public.comment_contract.b IS '列 中文';
SELECT c.oid,a.attnum FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid
WHERE n.nspname='public' AND c.relname='comment_contract' AND a.attname='b';
-- $1=上式真实表OID(26)，不是手填用户对象常量
SELECT objoid,classoid,objsubid,description FROM pg_catalog.pg_description
WHERE objoid=$1 AND classoid=1259 ORDER BY objsubid;
-- $1=表OID(26)，$2=上述attnum转int4(23)，$3='pg_class'(name/19)
SELECT pg_catalog.obj_description($1,$3),pg_catalog.col_description($1,$2);
-- COM-SHARED：owner
COMMENT ON DATABASE comment_db IS '库 中文';
SELECT d.oid,s.classoid,s.description FROM pg_catalog.pg_database d
LEFT JOIN pg_catalog.pg_shdescription s ON s.objoid=d.oid AND s.classoid=1262
WHERE d.datname='comment_db';
-- $1=上式数据库OID(26)，$2='pg_database'(name/19)
SELECT pg_catalog.shobj_description($1,$2);
```

| 验收ID | 行/类型、真实变化及拒绝边界 |
| --- | --- |
| COM-ROWS/KEY/FUNC | 取键查询唯一行(oid/26,int2/21)；描述查询恰两行：(真实表OID,1259,0,表注释)、(同OID,1259,真实b列号,列注释)，类型26/26/23/25；函数一行两列text/25，与内容逐字相同，obj_description不取列注释。当前函数未接线应绑定0A000，不返NULL伪装实现。 |
| COM-TEXT | owner执行下面多行COMMENT，再重跑描述/函数探针，目标逐字节保留真实换行、引号、反斜杠、中文、é和emoji；以 `SELECT description FROM pg_catalog.pg_description WHERE objoid=$1 AND classoid=1259 AND objsubid=0 AND description=$2;` 绑定表OID(26)与完全相同UTF-8内容(25)，目标一行原文text/25；比较text/binary，不折叠Unicode。 |
| COM-EMPTY | `COMMENT ON COLUMN public.comment_contract.b IS '';` 后重跑描述探针仅剩表行、col_description为NULL；重设列注释后再 `COMMENT ON COLUMN public.comment_contract.b IS NULL;` 结果相同。`COMMENT ON TABLE public.comment_contract IS NULL;` 后执行下面LEFT JOIN，仍一行(comment_contract,NULL)，证明无注释与空串输入删除一致、NULL扩展不是实体注释。 |
| COM-LIFE | 恢复表/列注释；`ALTER TABLE public.comment_contract RENAME COLUMN b TO renamed;` 后取键查询把attname改成renamed，OID/属性号/注释不变。`ALTER TABLE public.comment_contract DROP COLUMN a;` 后幸存列号不变，不能重排错配；缺槽位沿列契约DDL门禁。保存旧OID后 `DROP TABLE public.comment_contract;`，旧OID描述零行、obj_description为NULL；同名重建无继承注释。 |
| COM-SHARED | 共享查询一行(真实数据库OID,1262,'库 中文')、类型26/26/25，函数text/25同值；从comment_other连接重跑仍可读共享描述，本库pg_description不产生数据库描述。当前缺共享来源时COMMENT副作用前0A000；无注释数据库LEFT JOIN为一行NULL不能证明共享写入。角色/表空间各缺来源写入同样门禁。 |
| COM-ACCESS | owner在有注释表上 `CREATE ROLE comment_reader LOGIN; REVOKE ALL ON TABLE public.comment_contract FROM PUBLIC;`，`GRANT CONNECT ON DATABASE comment_db TO comment_reader; GRANT SELECT ON pg_catalog.pg_class,pg_catalog.pg_namespace,pg_catalog.pg_attribute,pg_catalog.pg_description,pg_catalog.pg_shdescription TO comment_reader;`，由reader用上述数字OID探针读取，目标仍可见。reader执行 `COMMENT ON TABLE public.comment_contract IS 'forbidden';` 目标42501，owner重读未变。无真实角色授权来源报告门禁，不能冒充权限通过；名称USAGE限制与数字OID读取分开。 |
| COM-SNAPSHOT | owner先Parse描述查询，另一连接提交替换注释，再Bind/首次Execute见新值；已执行portal分段读取不混代际。在comment_other建同名不同注释表，重跑本库查询不得混入其他库描述；事务边界沿第5章能力门槛，不据此声称支持事务型COMMENT。 |
| COM-REJECT | `SELECT objsubid FROM pg_catalog.pg_shdescription;` 目标绑定0A000；`SELECT description FROM pg_catalog.pg_description WHERE objoid=$1 AND classoid=1259 AND objsubid=0;` 绑定已删除对象旧OID(26)目标零行，Describe仍text/25。缺完整模型/codec须成功行前拒绝，不能以空集替代实现缺失。 |

```sql
-- COM-TEXT：字面量内是真实换行
COMMENT ON TABLE public.comment_contract IS '第二行
中文 é 😀 ''引号'' \';
-- COM-EMPTY：owner清除表注释后，无参数[]
SELECT c.relname,d.description FROM pg_catalog.pg_class c
LEFT JOIN pg_catalog.pg_description d
ON d.objoid=c.oid AND d.classoid=1259 AND d.objsubid=0
WHERE c.relname='comment_contract';
```

清理：owner分别连接两库DROP TABLE IF EXISTS public.comment_contract，移除reader权限依赖后DROP ROLE comment_reader，再切回维护库DROP DATABASE comment_db及comment_other。所有对象仅在本用例专用环境创建。成功SQLSTATE不适用；0A000/42501为待验收目标，完整参考错误字段与18.6行为未采集。

#### 角色权限与所有权契约

以下均为人工契约探针、**待新增/待运行**，无当前通过结论。使用第1章证据格式，默认数据库catalog_role_db、UTF8、search_path=public、空闲事务、参数[]、Q文本结果；每条还需Parse/Bind/Describe/Execute/Sync文本及二进制重放并采集逐列OID/格式/NULL、错误阶段及恢复后的 `SELECT 1`。DDL/角色命令是未来参考端/目标夹具准备输入，当前缺能力须0A000，不能跳过夹具再报成功。

管理员准备（待运行）：

```sql
CREATE ROLE catalog_owner LOGIN;
CREATE ROLE catalog_reader LOGIN;
CREATE ROLE catalog_none LOGIN;
CREATE ROLE catalog_group NOLOGIN;
CREATE SCHEMA catalog_acl AUTHORIZATION catalog_owner;
CREATE TABLE catalog_acl.t (id integer, secret text);
ALTER TABLE catalog_acl.t OWNER TO catalog_owner;
CREATE SEQUENCE catalog_acl.s;
ALTER SEQUENCE catalog_acl.s OWNER TO catalog_owner;
REVOKE ALL ON SCHEMA catalog_acl FROM PUBLIC;
REVOKE ALL ON TABLE catalog_acl.t FROM PUBLIC;
REVOKE ALL ON SEQUENCE catalog_acl.s FROM PUBLIC;
GRANT USAGE ON SCHEMA catalog_acl TO catalog_reader;
GRANT SELECT (id) ON catalog_acl.t TO catalog_reader;
```

| 用例 | 完整查询/参数与权限前提 | 目标及元数据变化；缺能力策略 |
| --- | --- | --- |
| ROLE-ID | 任意已登录最小权限角色：`SELECT usename, usesysid, usesuper FROM pg_catalog.pg_user WHERE usename = 'catalog_reader'`；`SELECT oid, rolname, rolcanlogin FROM pg_catalog.pg_roles WHERE rolname IN ('catalog_reader','catalog_group') ORDER BY rolname` | 前者一行name/19、oid/26、bool/16且false；后者两行，group=false、reader=true，同名reader键一致，NOLOGIN不在pg_user。跨库/连接键稳定；同名不同Host及含@身份需独立冲突夹具，未定义映射时0A000。 |
| ROLE-PSQL | `SELECT r.rolname,r.rolsuper,r.rolinherit,r.rolcreaterole,r.rolcreatedb,r.rolcanlogin,r.rolconnlimit,r.rolvaliduntil,r.rolreplication,r.rolbypassrls FROM pg_catalog.pg_roles r WHERE r.rolname='catalog_reader'` | psql官方字段子集的人工定值探针，不冒充完整生成SQL；一行name/19、五列bool/16、int4/23、timestamptz/1184、两列bool/16按列序描述，属性与显式准备策略一致；后续 `psql -E` 采集完整\du含过滤/排序及版本。未开放角色provider时0A000。 |
| ROLE-OWNER | reader：`SELECT c.relowner, pg_catalog.pg_get_userbyid(c.relowner) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='catalog_acl' AND c.relname='t'` | 一行oid/26、name/19，owner为catalog_owner，不能为reader；管理员 `ALTER TABLE catalog_acl.t OWNER TO catalog_group` 后新执行显示group、原表OID不变；Parse前后及第二连接复查，不使用portal缓存代替新执行。缺持久owner拒绝0A000。 |
| ROLE-PRIV | reader：`SELECT pg_catalog.has_table_privilege('catalog_reader','catalog_acl.t','SELECT'), pg_catalog.has_column_privilege('catalog_reader','catalog_acl.t','id','SELECT'), pg_catalog.has_column_privilege('catalog_reader','catalog_acl.t','secret','SELECT'), pg_catalog.has_any_column_privilege('catalog_reader','catalog_acl.t','SELECT')` | 四列bool/16，初态f,t,f,t；将四处显式用户参数均替换为catalog_none后全f，owner按真实owner权限判断。管理员 `GRANT SELECT ON catalog_acl.t TO catalog_reader` 后全t；`REVOKE SELECT ON catalog_acl.t FROM catalog_reader` 后回到f,t,f,t，列授权仍在。授权检查不能由目录行存在推断。 |
| ROLE-PRIV其他对象 | reader：`SELECT pg_catalog.has_schema_privilege('catalog_acl','USAGE'), pg_catalog.has_database_privilege(current_database(),'CONNECT'), pg_catalog.has_sequence_privilege('catalog_acl.s','USAGE'), pg_catalog.has_function_privilege('pg_catalog.pg_get_userbyid(oid)','EXECUTE'), pg_catalog.has_type_privilege('pg_catalog.int4','USAGE')` | 五列bool/16；schema=t、sequence=f，database/function/type依夹具明确有效默认ACL取真值并采证，禁止硬编码。随后显式GRANT/REVOKE逐项翻转并记录PUBLIC默认授权影响；函数与类型缺真实权限来源0A000。 |
| ROLE-MEMBER | 管理员 `GRANT catalog_group TO catalog_reader WITH INHERIT FALSE, SET TRUE`；reader：`SELECT session_user,current_user,current_role`；`SELECT pg_catalog.pg_has_role('catalog_group','MEMBER'),pg_catalog.pg_has_role('catalog_group','USAGE'),pg_catalog.pg_has_role('catalog_group','SET')`；再 `SET ROLE catalog_group` 并重查身份 | 初始三身份reader，成员结果t,f,t；SET后session_user仍reader、另两项group，RESET ROLE恢复。`SELECT roleid,member,grantor,admin_option,inherit_option,set_option FROM pg_catalog.pg_auth_members WHERE member=(SELECT oid FROM pg_catalog.pg_roles WHERE rolname='catalog_reader')` 对应真实授予边，列类型oid/26三列、bool/16三列；无成员模型或子查询能力时明确0A000，不补假边。 |
| ROLE-ACL | 管理员：`SELECT c.relacl FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='catalog_acl' AND c.relname='t'`；再 `GRANT SELECT ON catalog_acl.t TO catalog_reader WITH GRANT OPTION`、`REVOKE GRANT OPTION FOR SELECT ON catalog_acl.t FROM catalog_reader` | 一行aclitem[]/1034，grantor/grantee对应真实角色；配对 `SELECT pg_catalog.has_table_privilege('catalog_reader','catalog_acl.t','SELECT WITH GRANT OPTION')` 从t到f但普通SELECT保留。另以新表默认ACL、显式空ACL夹具核对NULL与空数组，PUBLIC授予单测不混入角色成员边；缺ACL来源/codec拒绝0A000。 |
| ROLE-NULL | `SELECT pg_catalog.pg_get_userbyid(NULL::oid),pg_catalog.has_table_privilege(NULL::oid,'SELECT'),pg_catalog.has_schema_privilege(NULL::oid,'USAGE')`；扩展 `SELECT pg_catalog.has_table_privilege($1,$2)`，参数OID=[26,25]、值=[NULL,'SELECT'] | 三列name/19、bool/16、bool/16均NULL；参数化结果bool/16为NULL，不能返回t/f。再以参考端确认不存在的OID替代NULL（OID值由实际目录排除确认），权限查询目标NULL；未知userbyid的fallback文本以18参考构建补证，不能虚构成NULL。 |
| ROLE-ERROR/SAFE | 前两条由具备schema USAGE的reader执行，后两条由none执行：`SELECT pg_catalog.has_table_privilege('catalog_acl.absent','SELECT')`；`SELECT pg_catalog.has_table_privilege('catalog_acl.t','BOGUS')`；`SELECT passwd FROM pg_catalog.pg_user WHERE usename='catalog_reader'`；`SELECT rolpassword FROM pg_catalog.pg_authid` | 已实现目标分别在对象解析阶段拒绝不存在关系（目标42P01）、权限字符串校验拒绝（目标22023）、只得脱敏星号、无授权敏感读取42501；错误顺序/文本与18.6精确字段待采。产品尚未提供能力时输出前0A000，不发成功行，不在日志/错误携带凭据、哈希或无权元数据。 |

最小权限连接、不同host映射、无权限对象、名称不存在、OID不存在、NULL及授权变化分别留证；表列SELECT不代表schema USAGE或登录CONNECT，改变权限不能替换会话认证。原生入口若仍仅允许bootstrap root，记录能力门禁和未运行多角色测试，禁止用root结果替代。准备SQL由专用管理员连接运行，清理需先RESET ROLE，再删除catalog_acl对象/schema，最后删除四个夹具角色；只清理本夹具。本轮未执行任何SQL。

#### 可见性与名称遮蔽契约

以下VIS-*均为**待新增/待运行**人工用例。本轮没有SQL、客户端或18.6参考运行。前提：隔离当前库、UTF8、autocommit、owner、目录SELECT/函数EXECUTE；每条DDL独立发送并提交。Q参数[]、文本结果；用户OID按本端映射，不比较跨端数值。分别准备 `CREATE TABLE public.pg_class (marker integer);`、`INSERT INTO public.pg_class VALUES (71);`。先以显式目录查询保存U=public.pg_class真实OID、C=pg_catalog.pg_class的OID（PG为1259）：

```sql
SELECT n.nspname,c.oid,c.relname,pg_catalog.pg_table_is_visible(c.oid) AS visible
FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
WHERE c.relname='pg_class' ORDER BY n.nspname;
```

BASE描述为name19/oid26/name19/bool16，长度64/4/64/1（name标准构建），typmod均−1。所有测试同时核对真实模型ID、目录行、普通查询与regclass，而非仅布尔。

| 用例/完整输入 | 期望与拒绝/权限前提 |
| --- | --- |
| VIS-PATH | 依次 `SET search_path TO public;`、`SET search_path TO public,pg_catalog;`、`SET search_path TO pg_catalog,public;`，每次运行BASE及 `SELECT 'pg_class'::regclass::oid,'public.pg_class'::regclass::oid,'pg_catalog.pg_class'::regclass::oid;`。BASE始终2行，可见性依次(C真/U假)、(C假/U真)、(C真/U假)；三列oid26依次(C,U,C)、(U,U,C)、(C,U,C)。中间路径 `SELECT marker FROM pg_class;` 恰71/int4；`SELECT oid FROM pg_catalog.pg_class WHERE oid=1259;` 始终1259/26。不得把用户表误送目录绑定。 |
| VIS-EMPTY/QUOTE | `SET search_path TO '';` 后BASE仍2行且C真/U假；`SELECT 'pg_class'::regclass::oid;` 得C，`SELECT 'public.pg_class'::regclass::oid;` 得U；`SELECT marker FROM pg_class;` 解析到目录并在列绑定拒绝。独立创建 `CREATE TABLE public."PG_Class" (marker integer); INSERT INTO public."PG_Class" VALUES (72);`，path=public,pg_catalog时 `SELECT marker FROM "PG_Class";` 得72；`SELECT '"PG_Class"'::regclass::oid,'pg_class'::regclass::oid;` 得其真实OID与U；大写引号名不得与小写合并，缺名称存储能力须DDL前0A000。 |
| VIS-DISPLAY/ARG | 对BASE两行运行 `SELECT c.oid,c.oid::regclass::text FROM pg_catalog.pg_class c WHERE c.relname='pg_class' ORDER BY c.oid;`；两列26/25，分别按三种PATH设置将输出文本重新作为regclass参数解析必须回原OID，被遮蔽项须含schema。`SELECT pg_catalog.pg_table_is_visible(NULL::oid);` 一行NULL/bool16；`SELECT pg_catalog.pg_table_is_visible(0::oid);` 目标一行NULL/bool16，依据PG18 namespace.c缺对象分支；当前未实测。错误参数数量 `SELECT pg_catalog.pg_table_is_visible();` 在绑定拒绝，无伪结果行；缺函数实现的0A000须单列为能力缺口。 |
| VIS-TYPE/FUNC | `SELECT pg_catalog.pg_type_is_visible(t.oid) FROM pg_catalog.pg_type t WHERE t.oid=23;` 在路径public时一行true/bool16；`SELECT p.oid,pg_catalog.pg_function_is_visible(p.oid) FROM pg_catalog.pg_proc p WHERE p.proname='pg_table_is_visible';` 内建签名对应一行26/16且true。后续用户重载夹具沿函数目录PROC-03，用两个schema同名同参/不同参分别验证按签名遮蔽；当前无用户注册来源时先拒绝DDL，不把缺夹具当通过。 |
| VIS-ACCESS | 角色创建/真实认证沿角色块。owner授予reader目录读取及public USAGE，撤销public.pg_class的SELECT；reader设path=public,pg_catalog，BASE U仍true，`SELECT marker FROM pg_class;` 权限拒绝且无71。另建schema vis_hidden、表vis_hidden.t并授reader表SELECT但无schema USAGE，reader设path=vis_hidden,public；`SELECT pg_catalog.pg_table_is_visible(c.oid) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='vis_hidden' AND c.relname='t';` 一行false，显式 `SELECT * FROM vis_hidden.t;` 权限拒绝。精确诊断参考采集，缺权限源不能冒充通过。 |
| VIS-LIFE/CONNECTION | A保持public,pg_catalog、B保持pg_catalog,public，交错PATH/BASE，分别见U/C；A RESET不得改变B，重连不继承A。owner `ALTER TABLE public.pg_class RENAME TO vis_renamed;` 后U身份不变，BASE只剩C/true；`SELECT 'pg_class'::regclass::oid;` 两连接均C，新名解析U。DROP重建后旧U不能指向新对象。准备BASE后改path/DDL再Bind/Execute，核对第2/5章绑定时点、元数据刷新及连接隔离，不能仅靠current_schema测试推导。 |

扩展参数探针完整SQL `SELECT pg_catalog.pg_table_is_visible($1);`，Parse参数OID[26]，Bind分别[U]、[C]、[NULL]，输入text格式[0]，结果各跑text[0]/binary[1]；Describe bool16/长度1/typmod−1，NULL仍有描述。每次记录当时路径、真实角色和快照，Parse/Describe不得改变路径或元数据；错误后Sync/ROLLBACK按第5/7章恢复。psql18后续以 `psql -E` 分别运行 `\d`、`\dt public.*`、`\dT`、`\df` 保存版本、真实SQL与结果，限定/未限定pattern分别检查；其结果均未采。

清理使用实际已创建名：owner `DROP TABLE public.pg_class;`（改名组改为vis_renamed）、`DROP TABLE public."PG_Class";`、`DROP TABLE vis_hidden.t; DROP SCHEMA vis_hidden;`；关闭statement/portal并RESET路径，角色按权限块清理。缺schema/引号/函数时保留门禁证据和未运行项，不跳过后报成功。

#### 标准模式表视图映射

以下IS-*均为**待新增/待运行**人工契约探针，Q参数[]、文本结果；本轮未执行。前提为隔离当前库D、UTF8、autocommit、owner，具有目录SELECT。各用例独立重置基线；reader无owner角色成员资格、无PUBLIC或其他角色继承的表/列权限。每条DDL独立发送提交：`CREATE TABLE public.is_map_t (id integer);`、`CREATE VIEW public.is_map_v AS SELECT id FROM public.is_map_t;`。缺视图/权限/schema能力按公共门禁记录，不跳过后宣称运行通过。

```sql
-- IS-BASE-S
SELECT catalog_name,schema_name,schema_owner
FROM information_schema.schemata WHERE schema_name='public';
-- IS-BASE-T
SELECT table_catalog,table_schema,table_name,table_type
FROM information_schema.tables WHERE table_schema='public'
AND table_name IN ('is_map_t','is_map_v') ORDER BY table_name;
-- IS-BASE-V
SELECT table_catalog,table_schema,table_name,view_definition
FROM information_schema.views WHERE table_schema='public' AND table_name='is_map_v';
-- IS-BASE-C：同一对象对照，不能由名称独立造行
SELECT n.nspname,c.oid,c.relname,c.relkind FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
WHERE n.nspname='public' AND c.relname IN ('is_map_t','is_map_v') ORDER BY c.relname;
```

BASE-S恰(D,public,真实owner)；BASE-T恰(D,public,is_map_t,BASE TABLE)、(D,public,is_map_v,VIEW)；BASE-V一行且owner定义重建等价SELECT。BASE-C同两对象真实OID、r/v；跨端用户OID不比较数值。描述S为19/19/19，T为19/19/19/1043，V为19/19/19/1043，C为19/26/19/18；域与完整wire元数据按目标类型段采集。

| 用例与完整输入/操作 | 预期及必要前提 |
| --- | --- |
| IS-COLS | `SELECT default_character_set_catalog,default_character_set_schema,default_character_set_name,sql_path FROM information_schema.schemata WHERE schema_name='public';` 一行四NULL，类型19/19/19/1043。`SELECT self_referencing_column_name,reference_generation,user_defined_type_catalog,user_defined_type_schema,user_defined_type_name,is_insertable_into,is_typed,commit_action FROM information_schema.tables WHERE table_schema='public' AND table_name='is_map_t';` 一行(NULL,NULL,NULL,NULL,NULL,YES,NO,NULL)，类型19/1043/19/19/19/1043/1043/1043。 |
| IS-FLAGS | `SELECT check_option,is_updatable,is_insertable_into,is_trigger_updatable,is_trigger_deletable,is_trigger_insertable_into FROM information_schema.views WHERE table_schema='public' AND table_name='is_map_v';` PG目标为(NONE,YES,YES,NO,NO,NO)，六列1043，简单单表视图具备PG自动更新条件且无trigger；若本端缺可更新分析必须输出前门禁，不把NO作为通过。独立 `CREATE VIEW public.is_map_checked AS SELECT id FROM public.is_map_t WHERE id>0 WITH LOCAL CHECK OPTION;` 后同查询将名称换is_map_checked，check_option=LOCAL；缺DDL语义在副作用前拒绝。 |
| IS-ACL/DEF | 按角色权限块建立实际reader认证，owner `GRANT USAGE ON SCHEMA public TO is_reader; GRANT SELECT ON public.is_map_v TO is_reader;`（逐条发送）；reader BASE-V一行但definition=NULL，BASE-T只含v，BASE-C仍可枚举t。`REVOKE SELECT ON public.is_map_v FROM is_reader;` 后reader BASE-V零行，owner仍一行。只授 `GRANT SELECT(id) ON public.is_map_t TO is_reader;` 后reader BASE-T含t，覆盖列授权；授schema USAGE不自动暴露未授权表。schema枚举、关系行过滤和定义NULL分别验证。 |
| IS-EMPTY | 在无用户对象的隔离新库E连接查询 `SELECT table_name FROM information_schema.tables WHERE table_schema='public';` 及 `SELECT table_name FROM information_schema.views WHERE table_schema='public';` 均零行但描述19；BASE-S仍列有权限的public。全库查询仍可含系统对象，不能断言整个tables为空。有用户schema能力后 `CREATE SCHEMA is_empty;` 并授reader USAGE，`SELECT schema_name FROM information_schema.schemata WHERE schema_name='is_empty';` 一行，即使其中无表。 |
| IS-DB/PATH | 另一库B创建同名is_map_t（通过B连接），D的BASE仅D对象；A/B交错执行不泄漏。`SET search_path TO pg_catalog;` 后显式BASE结果不变。具备用户schema后建is_other.is_map_t再运行 `SELECT table_schema,table_name FROM information_schema.tables WHERE table_name='is_map_t' ORDER BY table_schema;` owner得is_other/public两行，以schema区分，不能名称去重。 |
| IS-LIFE | 保存BASE-C的U/V；`ALTER TABLE public.is_map_t RENAME TO is_map_renamed;` 后BASE-T只剩v，`SELECT table_name FROM information_schema.tables WHERE table_schema='public' AND table_name='is_map_renamed';` 一行19，BASE-C用新名仍U。BASE-V定义须反映真实依赖重命名；无绑定来源门禁。删视图、删改名表、重建同名表后旧U不得指新对象，跨连接下次快照刷新；复用关系生命周期而非另建ID。 |
| IS-KIND | 具备对应真实能力后独立创建 `CREATE SEQUENCE public.is_map_s;`、`CREATE INDEX is_map_i ON public.is_map_t(id);`；`SELECT table_name FROM information_schema.tables WHERE table_schema='public' AND table_name IN ('is_map_s','is_map_i');` 零行、描述19；实际分区父表映射BASE TABLE。临时表、外表、物化视图由对应实施夹具补充（待新增），未开放种类在DDL前门禁，不用虚构行通过。 |

扩展探针：`SELECT table_catalog,table_schema,table_name,table_type FROM information_schema.tables WHERE table_schema=$1 AND table_name=$2;`，Parse参数OID[25,25]、Bind[public,is_map_t]，参数text[0]，结果text[0]/binary[1]分别验证一行；换第二参数missing得零行、NULL得零行，描述保持19/19/19/1043。Parse后另连接改名再首次Execute按第5章绑定/快照规则核对，不缓存Parse时目录行。`SELECT unknown_field FROM information_schema.tables;` 缺字段在绑定拒绝；已识别不支持的混合目录JOIN须明确门禁，不能落原生MySQL返回异义列。错误后Sync/ROLLBACK按第5/7章恢复。

客户端后续用psql18直接执行BASE并保存版本、SQL及描述；-E采集其元命令只作对照，不声称元命令使用标准视图。pgJDBC固定版本后经Statement/PreparedStatement运行同一探针，保存ResultSetMetaData、NULL/零行及prepareThreshold前后格式；DataGrip/ORM若使用本视图必须另采真实SQL，当前未验证。清理按实际创建名DROP视图/表/序列/schema及角色，先撤授权，关闭portal并RESET路径；不因夹具未开放而伪造结果。

#### 标准模式列映射

以下ISC-*全部为**待新增/待运行**人工探针，Q参数[]、文本结果，权限前提为隔离库D、UTF8、autocommit、owner及目录SELECT。DDL逐条提交，失败按第5章恢复；未开放DDL须副作用前拒绝并记录门槛，不能跳过后报通过。基线：

```sql
CREATE TABLE public.is_col_t (id integer NOT NULL, amount numeric(12,3) DEFAULT 1.250, label varchar(7) DEFAULT 'hello', stamp timestamp(3), plain integer);
-- ISC-BASE
SELECT table_catalog,table_schema,table_name,column_name,ordinal_position,is_nullable,data_type,udt_catalog,udt_schema,udt_name,column_default
FROM information_schema.columns WHERE table_schema='public' AND table_name='is_col_t' ORDER BY ordinal_position;
-- ISC-CROSS：在同一已提交模型版本核对
SELECT a.attnum,a.attname,a.atttypid,a.atttypmod,a.attnotnull,pg_catalog.pg_get_expr(d.adbin,a.attrelid)
FROM pg_catalog.pg_attribute a LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum
WHERE a.attrelid='public.is_col_t'::regclass::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum;
-- ISC-SIZE
SELECT column_name,character_maximum_length,character_octet_length,numeric_precision,numeric_precision_radix,numeric_scale,datetime_precision
FROM information_schema.columns WHERE table_schema='public' AND table_name='is_col_t' ORDER BY ordinal_position;
```

BASE恰5行，catalog均D、schema均public、table均is_col_t；名字/顺序id/amount/label/stamp/plain，位置1..5，可空NO/YES/YES/YES/YES；data_type依次integer/numeric/character varying/timestamp without time zone/integer，UDT为D/pg_catalog及int4/numeric/varchar/timestamp/int4。默认分别NULL、等价1.250、等价带类型hello常量、NULL、NULL，不比较反解空格。描述OID为19/19/19/19/23/1043/1043/19/19/19/1043。CROSS声明OID23/1700/1043/1114/23，名字、位置、nullable、默认须一致，用户关系OID不跨端硬比。SIZE除名字外六项：id/ plain=(NULL,NULL,32,2,0,NULL)，amount=(NULL,NULL,12,10,3,NULL)，label=(7,28,NULL,NULL,NULL,NULL)，stamp=(NULL,NULL,NULL,NULL,NULL,3)；描述19后六个23，不能以数值串text描述代替。

| 用例及完整输入/变化 | 期望及拒绝门槛 |
| --- | --- |
| ISC-DEF/LIFE | 逐条 `ALTER TABLE public.is_col_t ALTER COLUMN amount SET DEFAULT 2.500;`、`ALTER TABLE public.is_col_t ALTER COLUMN label SET NOT NULL;` 后BASE相应默认/nullable刷新；`ALTER TABLE public.is_col_t ALTER COLUMN amount DROP DEFAULT;` 后NULL。独立 `ALTER TABLE public.is_col_t ALTER COLUMN plain SET DEFAULT NULL;` 后对照CROSS与参考表达式，不把无默认作为预设。`ALTER TABLE public.is_col_t DROP COLUMN label;`、`ALTER TABLE public.is_col_t ADD COLUMN added varchar(2);` 后BASE剩位置1/2/4/5/6、末列added，不能按原生Offset压缩；随后改名 `ALTER TABLE public.is_col_t RENAME COLUMN added TO renamed;` 位置仍6。原生位置保真缺口未修前门禁，不把当前压缩断言当PG目标。 |
| ISC-ACL | reader按角色块真实认证、无owner继承/PUBLIC权限；owner `GRANT USAGE ON SCHEMA public TO is_reader;` 后reader BASE零行；`GRANT SELECT(id) ON public.is_col_t TO is_reader;` 后仅id一行，位置1；`REVOKE SELECT(id) ON public.is_col_t FROM is_reader;` 后零行；`GRANT SELECT ON public.is_col_t TO is_reader;` 后全部5列（此用例独立重置基线）。描述恒定；不能因tables可见而暴露未授权列。 |
| ISC-IDENTITY | 独立 `CREATE TABLE public.is_col_i (id bigint GENERATED BY DEFAULT AS IDENTITY (START WITH 7 INCREMENT BY 2 MINVALUE 1 MAXVALUE 99 NO CYCLE), plain integer);`；`SELECT column_name,is_identity,identity_generation,identity_start,identity_increment,identity_minimum,identity_maximum,identity_cycle,column_default FROM information_schema.columns WHERE table_schema='public' AND table_name='is_col_i' ORDER BY ordinal_position;` 得(id,YES,BY DEFAULT,7,2,1,99,NO,NULL)、(plain,NO,NULL,NULL,NULL,NULL,NULL,NULL,NULL)，19后八列1043。`ALTER TABLE public.is_col_i ALTER COLUMN id SET GENERATED ALWAYS;` 后ALWAYS。关联真实序列/依赖沿既有块核对；缺DDL能力副作用前拒绝。 |
| ISC-GEN | 独立 `CREATE TABLE public.is_col_g (a integer, b integer GENERATED ALWAYS AS (a+1) STORED);`；`SELECT column_name,column_default,is_generated,generation_expression,is_updatable FROM information_schema.columns WHERE table_schema='public' AND table_name='is_col_g' ORDER BY ordinal_position;` 得(a,NULL,NEVER,NULL,YES)、(b,NULL,ALWAYS,语义等价a+1,YES)，19/1043/1043/1043/1043。`CREATE VIEW public.is_col_v AS SELECT a,a+1 AS calculated FROM public.is_col_g;` 后同查询改表名为is_col_v，is_updatable为YES/NO，不能套用调用者权限；缺绑定反解/视图分析门禁。 |
| ISC-TYPE | 对应能力开放后逐条 `CREATE DOMAIN public.is_col_dom AS integer NOT NULL;`、`CREATE TABLE public.is_col_types (d public.is_col_dom, a integer[], dt date, f real);`；BASE换表名得data_type integer/ARRAY/date/real、UDT int4/_int4/date/float4，d可空NO。`SELECT column_name,domain_catalog,domain_schema,domain_name FROM information_schema.columns WHERE table_schema='public' AND table_name='is_col_types' ORDER BY ordinal_position;` 仅d域三项D/public/is_col_dom，其余NULL、四列19。SIZE换表名：dt精度0，f数值精度24/radix2/scale NULL；域精度来自底层。未开放域/数组在DDL门禁，不生成伪类型行。 |
| ISC-EXTRA | `SELECT column_name,character_set_catalog,character_set_schema,character_set_name,scope_catalog,scope_schema,scope_name,maximum_cardinality,is_self_referencing,collation_catalog,collation_schema,collation_name,dtd_identifier,interval_type,interval_precision FROM information_schema.columns WHERE table_schema='public' AND table_name='is_col_t' ORDER BY ordinal_position;` 基线每行字符集/scope/最大基数/默认collation/interval两项NULL，self=NO，dtd非空且同表唯一；描述依次19、六个19、23、1043、四个19、1043、23。显式PG排序规则/interval夹具由既有类型实施补充（待新增），不能用原生名称猜测。 |

扩展输入：`SELECT column_name,ordinal_position,is_nullable,data_type FROM information_schema.columns WHERE table_schema=$1 AND table_name=$2 ORDER BY ordinal_position;` Parse OID[25,25]，Bind[public,is_col_t]、参数text[0]，结果text[0]/binary[1]分别核对5行及描述19/23/1043/1043；第二参数missing或NULL均零行、描述不变。另一连接ALTER已提交后重新Bind/Execute刷新行，已打开portal沿第5章快照规则，不缓存Parse时目录行。另一库同名表、显式查询且search_path改pg_catalog均不泄漏/改变D的对象集。`SELECT unknown_field FROM information_schema.columns;` 绑定拒绝；Sync/ROLLBACK恢复沿公共契约。

psql18直接运行探针并保存版本/SQL/描述；固定pgJDBC版本用Statement及PreparedStatement（JDBC占位符?）运行等价参数查询，保存ResultSetMetaData、NULL/零行及prepareThreshold前后结果。本轮无客户端运行证据，现有RetrieveColumns夹具不能替代；18.6需独立构建身份。清理实际创建的视图、表、域和角色授权，关闭portal并RESET路径，各用例独立恢复基线。

#### 标准模式约束映射

以下ISCON-*均**待新增/待运行**，人工探针Q参数[]、文本结果；隔离库D、UTF8、owner、目录SELECT、autocommit，DDL逐条提交。未开放能力须副作用前0A000且无残留，不跳过后报通过。准备：

```sql
CREATE TABLE public.iscon_p (a integer, b integer, CONSTRAINT iscon_pk PRIMARY KEY(a,b));
CREATE TABLE public.iscon_c (x integer, y integer, v integer,
  CONSTRAINT iscon_fk FOREIGN KEY(x,y) REFERENCES public.iscon_p(b,a) ON UPDATE RESTRICT ON DELETE CASCADE,
  CONSTRAINT iscon_ck CHECK(v > 0));
-- ISCON-BASE：下列每条均独立Q，参数[]
SELECT constraint_catalog,constraint_schema,constraint_name,table_catalog,table_schema,table_name,constraint_type,is_deferrable,initially_deferred,enforced,nulls_distinct
FROM information_schema.table_constraints WHERE table_schema='public' AND table_name IN ('iscon_p','iscon_c') ORDER BY table_name,constraint_name;
SELECT constraint_catalog,constraint_schema,constraint_name,table_catalog,table_schema,table_name,column_name,ordinal_position,position_in_unique_constraint
FROM information_schema.key_column_usage WHERE table_schema='public' AND table_name IN ('iscon_p','iscon_c') ORDER BY table_name,constraint_name,ordinal_position;
SELECT constraint_catalog,constraint_schema,constraint_name,unique_constraint_catalog,unique_constraint_schema,unique_constraint_name,match_option,update_rule,delete_rule
FROM information_schema.referential_constraints WHERE constraint_schema='public' AND constraint_name='iscon_fk';
SELECT table_catalog,table_schema,table_name,column_name,constraint_catalog,constraint_schema,constraint_name
FROM information_schema.constraint_column_usage WHERE constraint_schema='public' AND constraint_name IN ('iscon_pk','iscon_fk','iscon_ck') ORDER BY constraint_name,column_name;
-- ISCON-JOIN：TC与KCU用约束加表身份，保留有序键
SELECT t.table_schema,t.table_name,t.constraint_name,k.column_name,k.ordinal_position,k.position_in_unique_constraint
FROM information_schema.table_constraints t JOIN information_schema.key_column_usage k
ON t.constraint_catalog=k.constraint_catalog AND t.constraint_schema=k.constraint_schema AND t.constraint_name=k.constraint_name
AND t.table_catalog=k.table_catalog AND t.table_schema=k.table_schema AND t.table_name=k.table_name
WHERE t.table_schema='public' AND t.table_name='iscon_c' AND t.constraint_type='FOREIGN KEY' ORDER BY k.ordinal_position;
-- ISCON-CROSS：保留身份而非仅凭约束名验证
SELECT c.oid,c.conname,c.contype,c.conrelid,c.confrelid,c.conindid,c.conkey,c.confkey,c.condeferrable,c.condeferred,c.conenforced
FROM pg_catalog.pg_constraint c WHERE c.conrelid IN ('public.iscon_p'::regclass::oid,'public.iscon_c'::regclass::oid) ORDER BY c.conrelid,c.oid;
```

BASE预期：catalog均D，约束/表schema均public。TC中iscon_pk/p为PRIMARY KEY、iscon_fk/c为FOREIGN KEY、iscon_ck/c为CHECK；这三行状态NO/NO/YES，nulls_distinct均NULL；父a/b的PG18 NOT NULL约束另有CHECK行（实际名称从真实目录取，不硬编码），不能断言TC总共3行。KCU恰4行：pk的a/b位置1/2、unique位置NULL；fk的x/y位置1/2、unique位置2/1。RC恰1行，父D/public/iscon_pk、NONE/RESTRICT/CASCADE。CCU过滤这三名后恰5行：pk的父a/b，fk也为父a/b，ck为子v。TC描述六个19后五个1043；KCU七个19后两个23；RC六个19后三个1043；CCU七个19。JOIN恰两行public/iscon_c/iscon_fk及x/1/2、y/2/1，描述19/19/19/19/23/23。CROSS描述26/19/18/26/26/26/1005/1005/16/16/16，OID只比较本端，conkey/confkey配对与父索引顺序、TC状态一致；全输出Describe、NULL、零行与文本/二进制编码沿第3章另采，不以值正确替代类型正确。

各行独立恢复BASE夹具，不累积其他行的新增约束或重命名；所有替换仅改WHERE指定条件，保留原投影/排序。

| 用例→完整变化/输入 | 期望/拒绝及权限前提 |
| --- | --- |
| ISCON-ORDER→复合逆序与参数 | 对BASE的KCU查询将table_name IN条件换为`table_name=$1`、table_schema换为`table_schema=$2`，Parse OID=[25,25]，Bind=['iscon_c','public']文本，Describe/Execute/Sync；两行fk x/y仍为1/2及2/1。复用同一prepared statement在DDL后重执行，不能捕获旧快照；严格同序父索引现状不降低此目标。 |
| ISCON-INDEX→独立索引 | 独立夹具逐条`CREATE TABLE public.iscon_i (a integer,b integer);`、`CREATE UNIQUE INDEX iscon_ix ON public.iscon_i(a,b);`、`CREATE TABLE public.iscon_r (x integer,y integer,CONSTRAINT iscon_ifk FOREIGN KEY(x,y) REFERENCES public.iscon_i(a,b));`。BASE的RC改名称为iscon_ifk：一行，unique三项NULL、NONE/NO ACTION/NO ACTION；TC的表名过滤改iscon_i应0行，不造UNIQUE；KCU过滤iscon_r两行父索引位置1/2。 |
| ISCON-CHECK→真实表达式依赖 | `ALTER TABLE public.iscon_c ADD CONSTRAINT iscon_const CHECK(1=1);`、`ALTER TABLE public.iscon_c ADD CONSTRAINT iscon_multi CHECK(v > x AND v > 0);` 后，BASE的CCU名称过滤改为这两名：const零行、multi恰x/v两行。TC两行CHECK，KCU无这两约束行。CHECK未开放则DDL前拒绝，不使用空集掩盖已存在表达式。 |
| ISCON-STATE→UNIQUE与延迟 | 独立`CREATE TABLE public.iscon_u (a integer, b integer, CONSTRAINT iscon_ud UNIQUE(a), CONSTRAINT iscon_un UNIQUE NULLS NOT DISTINCT(b) DEFERRABLE INITIALLY DEFERRED);`。TC表名过滤改iscon_u：两行UNIQUE，ud状态NO/NO/YES/YES，un为YES/YES/YES/NO（末项nulls_distinct）；KCU均1/NULL。NOT VALID和enforced分离沿CON-VALIDATE，不能将convalidated=false映成enforced=NO。缺执行/状态来源在DDL前拒绝。 |
| ISCON-NAME→同名不串联 | 逐条`CREATE SCHEMA iscon_s;`、`CREATE TABLE iscon_s.other (a integer,CONSTRAINT iscon_ck CHECK(a>0));`、`CREATE TABLE public.iscon_other (a integer,CONSTRAINT iscon_ck CHECK(a>0));`。TC查询去除BASE原WHERE，换为`WHERE constraint_name='iscon_ck'`；CCU同样只保留此WHERE，查询保留三对象（schema与table共同区分）；同public两个同名约束不能按三元名称合并。`SET search_path TO iscon_s,public;` 后限定查询行集不变；无法消歧的RC名称联接改用pg_constraint身份，不保证标准视图提供不存在的OID列。 |
| ISCON-ACL→真实最小权限 | 管理员逐条`CREATE ROLE iscon_reader;`、`GRANT USAGE ON SCHEMA public TO iscon_reader;`、`GRANT SELECT(x) ON public.iscon_c TO iscon_reader;`，以真实有效角色reader运行四条BASE。TC/RC无子约束，KCU仅fk的x（位置1、unique位置2），CCU无父/子行。再`GRANT UPDATE(y) ON public.iscon_c TO iscon_reader;`，TC可见子CHECK/FK、RC可见fk，KCU见x/y，CCU仍无（reader不是父表owner）。管理员逐条`REVOKE SELECT(x) ON public.iscon_c FROM iscon_reader;`、`REVOKE UPDATE(y) ON public.iscon_c FROM iscon_reader;`后重新执行恢复空集；owner的BASE不变。角色激活/认证未开放时沿角色块门禁，不以root代测。 |
| ISCON-LIFE→真实刷新/隔离 | 逐条`ALTER TABLE public.iscon_c RENAME COLUMN x TO xx;`、`ALTER TABLE public.iscon_p RENAME COLUMN b TO bb;`：KCU子xx/y序位不变、CCU父a/bb、RC父身份不变；`ALTER TABLE public.iscon_c DROP CONSTRAINT iscon_fk;` 后KCU/RC/CCU相应行消失、父pk保留；重建同名FK须新真实约束身份。第二连接在每次提交后重执行BASE及prepared版，另一数据库同名对象不能串入D。事务内可见性沿第5章，不能用autocommit测试宣称事务DDL通过。 |

所有成功SQLSTATE不适用；产品能力拒绝为0A000，精确诊断文本/位置与18.6差分仍未采集。清理隔离夹具：管理员逐条`DROP TABLE public.iscon_r;`、`DROP TABLE public.iscon_i;`、`DROP TABLE public.iscon_c;`、`DROP TABLE public.iscon_p;`、`DROP TABLE public.iscon_u;`、`DROP TABLE public.iscon_other;`、`DROP TABLE iscon_s.other;`、`DROP SCHEMA iscon_s;`、`REVOKE USAGE ON SCHEMA public FROM iscon_reader;`、`DROP ROLE iscon_reader;`。只清理实际成功创建项，事务错误先沿第5章恢复。以上SQL/角色/DDL均是后续验收输入，本轮无运行通过记录。

#### 无原生对象的响应策略

以下ABS-*全部**待新增/待运行**，人工契约探针，不冒称客户端生成SQL。隔离库D、UTF8、owner且具目录SELECT、autocommit、文本Q参数[]；每条SELECT独立执行。EMPTY夹具逐条`CREATE TABLE public.abs_t (id integer);`、`INSERT INTO public.abs_t VALUES (1);`，无FDW/RLS/触发器/用户规则；FK/视图只在REQUIRED夹具加入。

```sql
-- ABS-EMPTY：每条零行但Describe分别保持目标字段表类型
SELECT oid,spcname,spcowner,spcacl,spcoptions FROM pg_catalog.pg_tablespace ORDER BY oid;
SELECT oid,fdwname,fdwowner,fdwhandler,fdwvalidator,fdwacl,fdwoptions FROM pg_catalog.pg_foreign_data_wrapper ORDER BY oid;
SELECT oid,srvname,srvowner,srvfdw,srvtype,srvversion,srvacl,srvoptions FROM pg_catalog.pg_foreign_server ORDER BY oid;
SELECT f.ftrelid,s.srvname,f.ftoptions FROM pg_catalog.pg_foreign_table f LEFT JOIN pg_catalog.pg_foreign_server s ON f.ftserver=s.oid ORDER BY f.ftrelid;
SELECT p.oid,p.polname,p.polrelid,p.polpermissive,p.polroles,p.polcmd,p.polqual,p.polwithcheck FROM pg_catalog.pg_policy p JOIN pg_catalog.pg_class c ON p.polrelid=c.oid WHERE c.relname='abs_t';
SELECT t.oid,t.tgrelid,t.tgname,t.tgfoid,t.tgisinternal FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON t.tgrelid=c.oid WHERE c.relname='abs_t';
SELECT r.oid,r.ev_class,r.rulename,r.ev_type,r.ev_enabled,r.is_instead,r.ev_qual,r.ev_action FROM pg_catalog.pg_rewrite r JOIN pg_catalog.pg_class c ON r.ev_class=c.oid WHERE c.relname='abs_t';
-- ABS-NULL：一条保留左表行；右侧两个NULL，类型19/19/1009
SELECT c.relname,s.srvname,f.ftoptions FROM pg_catalog.pg_class c LEFT JOIN pg_catalog.pg_foreign_table f ON c.oid=f.ftrelid LEFT JOIN pg_catalog.pg_foreign_server s ON f.ftserver=s.oid WHERE c.relname='abs_t';
-- ABS-UNKNOWN：分别42P01、42703，不可成功零行
SELECT oid FROM pg_catalog.pg_missing_tablespace;
SELECT missing_column FROM pg_catalog.pg_tablespace;
-- ABS-REQUIRED：列类型依目标字段表，缺事实须0A000，不能成功伪空
SELECT oid,collname,collnamespace,collowner,collprovider,collisdeterministic,collencoding,collcollate,collctype,colllocale FROM pg_catalog.pg_collation WHERE collname='default';
SELECT oid,opfname,opfmethod,opfowner,opfnamespace FROM pg_catalog.pg_opfamily WHERE opfname='integer_ops';
SELECT oid,opcname,opcmethod,opcintype,opckeytype,opcdefault,opcfamily,opcowner,opcnamespace FROM pg_catalog.pg_opclass WHERE opcname='int4_ops';
SELECT a.oid,a.amopfamily,a.amopstrategy,a.amopopr,a.amopsortfamily FROM pg_catalog.pg_amop a JOIN pg_catalog.pg_opfamily f ON a.amopfamily=f.oid WHERE f.opfname='integer_ops';
SELECT a.oid,a.amprocfamily,a.amprocnum,a.amproc,a.amproclefttype,a.amprocrighttype FROM pg_catalog.pg_amproc a JOIN pg_catalog.pg_opfamily f ON a.amprocfamily=f.oid WHERE f.opfname='integer_ops';
```

EMPTY仅证明受限无对象策略，不作为PG默认库全目录等价用例。REQUIRED的PG参考库应有default排序规则及内建整数族/类/成员；按method/namespace身份区分多行，不硬编码OID/跨端行号。当前目标实现若缺事实则拒绝，不能将拒绝计作该能力已支持。

ABS-PARAM：将EMPTY的policy查询WHERE换为`WHERE c.relname=$1`，Parse类型[25]、Bind文本['abs_t']、Describe/Execute/Sync，预期零行且描述26/19/26/16/1028/18/194/194；同一prepared查询每次Execute重新取快照。文本/二进制结果各一次，NULL及零行不降低类型要求，未有codec时Parse/描述前0A000。

ABS-DDL：owner分别提交`CREATE SERVER abs_srv FOREIGN DATA WRAPPER abs_missing;`、`CREATE POLICY abs_p ON public.abs_t USING (id>0);`、`ALTER TABLE public.abs_t ENABLE ROW LEVEL SECURITY;`、`CREATE TABLESPACE abs_sp LOCATION '/abs_contract_only';`。能力未开放须副作用前0A000（SERVER若已开放则未知wrapper按正常解析报错，不能成功）；重跑EMPTY及`SELECT id FROM public.abs_t;`仍无新对象、原行1不受影响。不实际创建宿主路径。RLS/FDW能力未来开放后用真实合法wrapper/角色夹具另立用例，不以这些拒绝探针验证完整功能。

ABS-LIFE/REQUIRED：逐条`CREATE VIEW public.abs_v AS SELECT id FROM public.abs_t;`、`CREATE TABLE public.abs_c (id integer REFERENCES public.abs_t(id));`（独立恢复：先`DROP TABLE public.abs_t;`，再`CREATE TABLE public.abs_t (id integer PRIMARY KEY);`、`INSERT INTO public.abs_t VALUES (1);`，随后才执行VIEW/FK建表）。将EMPTY的rewrite过滤换abs_v，应有真实视图规则关联或0A000；trigger过滤换abs_c，应有与FK关联的真实内部对象或0A000，不能伪空。相关DDL未开放则副作用前拒绝并记能力缺口。分别`ALTER TABLE public.abs_t RENAME TO abs_renamed;`后按新名重查NULL/相关目录，真实对象关联不能丢失；`DROP VIEW public.abs_v;`、`DROP TABLE public.abs_c;`后关联行消失。第二连接每次提交后重执行，另库同名对象不能串入D。DDL事务语义沿第5章，不据此宣称事务DDL通过。

ABS-ACL：管理员逐条`CREATE ROLE abs_reader;`、`GRANT USAGE ON SCHEMA public TO abs_reader;`、`GRANT SELECT ON public.abs_t TO abs_reader;`，以真实有效角色abs_reader与owner分别执行EMPTY/NULL；有目录读取权限且对象真实无行时同样空，管理员逐条`REVOKE SELECT ON pg_catalog.pg_tablespace FROM PUBLIC;`、`REVOKE SELECT ON pg_catalog.pg_tablespace FROM abs_reader;`后，abs_reader执行EMPTY首条应42501（独立隔离库，结束由管理员`GRANT SELECT ON pg_catalog.pg_tablespace TO PUBLIC;`恢复），不能伪空；对象使用权限不足不自动抹去公开目录。角色/授权能力未开放则门禁0A000，不以root代测。每个错误后Q执行`SELECT 1;`，扩展路径先Sync再SELECT 1，恢复沿第5/7章。清理仅成功创建项：`DROP VIEW public.abs_v;`、`DROP TABLE public.abs_c;`、`DROP TABLE public.abs_t;`（重命名后改用`DROP TABLE public.abs_renamed;`），随后`REVOKE USAGE ON SCHEMA public FROM abs_reader;`、`DROP ROLE abs_reader;`；服务器/策略/空间若意外创建须先记录失败并按官方对象依赖清理，不能据目录空假定无副作用。精确诊断文本、typlen/typmod、真实客户端SQL与18.6输出仍待采集。

#### 目录求值与普通查询边界

以下均为**人工契约探针，待新增独立测试、待运行**。默认参数[]、Q文本结果；扩展对照使用Parse→Describe(statement)→Bind→Describe(portal)→Execute→Sync，结果格式分别0/1。每个输出列需采OID/typlen/typmod/来源属性与NULL，沿第3章；未实现二进制编码须显式拒绝，不能伪造通过。角色为真实表owner，拥有schema USAGE及表SELECT；另以无该权限角色重复，预期依本章权限块判断，不能统一伪空。数据库固定、search_path=public,pg_catalog，初态I。

准备（目标夹具，未执行）：`CREATE TABLE public.route_t(id integer); INSERT INTO public.route_t VALUES (7);`。先保存真实table_oid与namespace_oid作为关联基准，不硬编码跨端OID。清理：`DROP TABLE public.route_t;`。

| ID | 完整SQL及参数 | 期望与拒绝阶段 |
| --- | --- | --- |
| J | `SELECT c.oid,n.nspname FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE c.relname='route_t' AND n.nspname='public'`；[] | 恰一行(真实table_oid,public)，OID26/19；LEFT JOIN空右关系变体沿前块类型NULL契约。 |
| C | `WITH x AS (SELECT oid FROM pg_catalog.pg_class WHERE relname='route_t') SELECT oid FROM x`；[] | 隔离夹具无同名关系时一行table_oid、26；重复引用同一CTE不得因消费耗尽丢行。 |
| S | `SELECT (SELECT oid FROM pg_catalog.pg_class WHERE relname='route_t') AS id FROM pg_catalog.pg_namespace WHERE nspname='public'`；[] | 一行table_oid、26；将内层过滤改为`relname='route_missing'`得一行NULL/26；再创建同名异schema关系使内层多行，Execute前无成功行，目标21000。相关子查询未开放在Parse拒绝0A000，不能忽略外层引用。 |
| A | `SELECT array_agg(oid ORDER BY oid) AS ids FROM pg_catalog.pg_class WHERE relname='route_t'`；[] | 一行含真实table_oid的oid[]/1028；过滤改为route_missing时NULL/1028。`SELECT count(*) FROM pg_catalog.pg_class GROUP BY relnamespace`未开放在分析阶段0A000；不得以0或空集模拟聚合。 |
| P | `SELECT c.oid FROM pg_catalog.pg_class c WHERE c.oid=$1::oid` | 扩展Parse参数[26]，Bind文本十进制真实table_oid或二进制网络序4字节，结果一行26；NULL参数结果零行但描述26。非数字文本在Bind拒绝22P02；Q同SQL无绑定拒绝。 |
| M | `SELECT c.oid,u.id FROM pg_catalog.pg_class c JOIN public.route_t u ON true WHERE c.relname='route_t'`；[] | 未接共享planner时Q分析/Parse拒绝0A000；逗号变体`SELECT c.oid,u.id FROM pg_catalog.pg_class c,public.route_t u`及`WITH u AS (SELECT id FROM public.route_t) SELECT c.oid FROM pg_catalog.pg_class c JOIN u ON true`同样拒绝，不回退后端重试。 |
| O | `SELECT id FROM public.route_t WHERE id=7`；`SELECT 'from pg_catalog.pg_class' AS literal`；[] | 普通共享SQL分别一行7/int4/23和字面量/text/25；`SELECT id FROM public.route_t /* from pg_catalog.pg_class */`仍共享执行。CTE名pg_class遮蔽探针沿第2/5章，不按词串接管。 |

数组展开补充探针：`SELECT ARRAY(SELECT unnest FROM unnest(c.conkey)) AS keys FROM pg_catalog.pg_constraint c WHERE c.conrelid=$1::oid ORDER BY c.oid`，Parse参数[26]、Bind为真实约束所属表OID。字段conkey来源及可见性沿约束块，元素int2/21、数组int2[]/1005；无约束时零行，有约束时按真实列序，不编造空键。局部数组语法/codec未开放时分析或编码前0A000，待新增带真实主键夹具验证，不以当前测试存在宣称支持。混合子查询拒绝探针：`SELECT c.oid FROM pg_catalog.pg_class c WHERE c.oid IN (SELECT id FROM public.route_t)`，参数[]，未接共享planner在Q分析/Parse拒绝0A000。

真实变更：保存J/P描述，`ALTER TABLE public.route_t RENAME TO route_renamed;`后新Execute用J的过滤改为route_renamed仍关联原table_oid，旧名称过滤零行且列类型不变；同一Execute快照内不得混用前后身份。重命名后清理改`DROP TABLE public.route_renamed;`，后续J零行。跨连接可见时点交批次21，不在此宣称验证。

预算/取消夹具待新增：按源码上限生成8/9个JOIN、16/17个CTE、24/25列CTE、8/9层查询的成对输入，保存生成后的完整SQL；边界内须验证真实结果，超界Parse错误。共享schema夹具逐步增加真实关系至累计物化超过16384行，及目录CROSS JOIN达到100000/100001次comparison，记录实际计数，不以最终LIMIT行数作证明。Execute越限不能返回部分成功；活跃执行期间发CancelRequest，原连接收到57014才算取消命中；空闲取消不得影响下一条。资源/取消错误后Sync回I、`SELECT 1`可用；显式BEGIN内错误后Sync仍E，ROLLBACK后再查询恢复，沿第5章。耗时/内存、预算边界实际可达性、精确错误映射均待取证。

#### 目录更新与连接隔离

以下CAT-REFRESH用例均待新增/待运行，无红绿或客户端通过记录。两PG连接A/B以同一owner连接专用库cat_refresh，初始I、UTF8、默认隔离；具有CONNECT、public USAGE/CREATE及目录读取权限。管理连接事务外逐条 `CREATE DATABASE cat_refresh;`、`CREATE DATABASE cat_refresh_other;`；C连接后 `CREATE TABLE public.refresh_probe (other_id integer);`，随后保持空闲；实际库名记录startup。A/B设置search_path为public,pg_catalog，C相同。所有SQL参数[]、文本结果；扩展Parse参数OID[]、Bind值[]/格式[]，逐列描述记录type OID/typmod/NULL及实际对象OID，不预造OID数值。跨版本name/19与当前text/25差异显式登记。

固定完整探针（人工编写，后续引用R/C/D均指以下原SQL）：

```sql
-- R：行类型依次 oid/26, name/19, oid/26
SELECT c.oid,c.relname,c.relnamespace FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE n.nspname='public' AND c.relname IN ('refresh_probe','refresh_renamed') ORDER BY c.relname;
-- C：行类型依次 oid/26, int2/21, name/19, oid/26, int4/23, bool/16
SELECT a.attrelid,a.attnum,a.attname,a.atttypid,a.atttypmod,a.attnotnull FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON a.attrelid=c.oid JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE n.nspname='public' AND c.relname IN ('refresh_probe','refresh_renamed') AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum;
-- D：name/19
SELECT current_database();
```

| ID及完整变化输入/顺序 | 目标观察与拒绝/恢复证据 |
| --- | --- |
| REF-CREATE：先A/B R/C均零行；A `CREATE TABLE public.refresh_probe (id integer NOT NULL, label varchar(7));`，成功完成后A/B各R/C | R一行(U,refresh_probe,N)；C为(U,1,id,23,-1,true)、(U,2,label,1043,11,false)。U/N现场记录，关联一致。C连接仍只见自己的other_id，D分别cat_refresh/cat_refresh_other；显式目录名不受另一连接path影响。创建若尚不支持，在写入前能力拒绝，登记差异，不改为原生SQL冒充PG通过。 |
| REF-ALTER：A `ALTER TABLE public.refresh_probe RENAME TO refresh_renamed;`；再 `ALTER TABLE public.refresh_renamed ADD COLUMN extra integer;`；再 `ALTER TABLE public.refresh_renamed DROP COLUMN label;`；每条完成后A/B R/C | rename保持U/N、旧名消失；ADD后新增(U,3,extra,23,-1,false)；DROP后活列仅id/extra且extra仍attnum3，不复用label槽位。R始终一对象；C不混入其他库列。PG语法入口、attnum差异均必须登记，不以原生测试意图判成功。 |
| REF-DROP：A `DROP TABLE public.refresh_renamed;` 后A/B R/C；再A `CREATE TABLE public.refresh_renamed (fresh bigint);` 后各R/C | DROP零行且保留完整描述；重建R新U2不等于U，C仅(U2,1,fresh,20,-1,false)。旧OID不能指向新对象。 |
| REF-SNAPSHOT：独立重复REF-CREATE夹具；在一次B C求值的provider取值边界暂停，A完成rename/ADD，再继续B | 测试同步钩子待新增，不用sleep猜时序。B只能返回完整旧/新版本；同查询重复provider、CTE/子查询按求值块扩展同一屏障；下一次B R/C必见发布版。记录SchemaMetaVersion仅作内部证据，不填输出列。已绑定名称消失的错误也须完整拒绝，不能部分成功。 |
| REF-PREPARE：A/B各Parse同名s=R，Describe statement、Sync；A完成REF-ALTER中rename；各Bind新portal p、Describe portal、Execute/Sync，再DROP后新Bind q/Execute | R描述保持三列固定类型；新执行依次见新名、零行。B Close自己的s不影响A；B Bind到不存在s须拒绝，Sync后SELECT 1 AS ok返回1/I，A仍可新Bind执行。精确缺statement诊断待采。已执行portal暂停续读另轮：B BEGIN、Bind p、Execute maxRows=1，A完成DDL，B续Execute只读首次结果，Sync后ROLLBACK；需至少两个匹配关系，该轮另预建 `CREATE TABLE public.refresh_renamed (id integer);`，A仅 `ALTER TABLE public.refresh_probe ADD COLUMN extra integer;`，避免rename碰撞，禁止把两个时刻混在同portal。 |
| REF-META：独立初始夹具，B Parse m=`SELECT * FROM public.refresh_probe`、Describe/Sync；A `ALTER TABLE public.refresh_probe ADD COLUMN extra integer;`；B Bind p/Describe/Execute/Sync | 普通SQL结果结构改变须重分析或明确拒绝，按第3/5章门槛；若拒绝，记录Bind或Execute实际阶段，不能将旧两列Describe配新三列DataRow。当前Execute描述不一致目标0A000，PG精确码/阶段未采；新Parse m2后描述三列且可执行。非目录普通SELECT不声称走CatalogQuery。 |
| REF-LOCAL：A `SET search_path TO pg_catalog;`，B保持public,pg_catalog，交错D/R/C及 `SHOW search_path;`；A断连重连原库，再C D/R/C | D始终各自启动库；显式R/C只按所属库取事实，B路径不变。A重连清除旧statement/portal，SHOW按启动默认而非旧A私值。角色权限负例复用角色权限块，不能从owner夹具推导无权限连接不会泄露。 |
| REF-TX：独立初始无refresh_probe；A BEGIN后 `CREATE TABLE public.refresh_probe (id integer);`，B R/C，A Sync（扩展组）再ROLLBACK，双方R/C | 当前产品应0A000/E、无对象发布，B始终零行，ROLLBACK后A恢复I及SELECT 1成功。未来开放时改用第5章COMMIT/ROLLBACK/失败清理完整观察轮次；未提交A/B可见性与锁等待需独立采证，不用本轮当前拒绝代替成功事务证明。 |

每轮从记录过的对象存在性恢复夹具，A/B/C先Sync（需要时）、ROLLBACK至I，由owner事务外删除实际存在的refresh_probe/refresh_renamed；断开目标库连接后管理连接删两专用库。临时加的第二关系同步清理。Q与扩展分别执行生命周期主链；拒绝记录S/V/C/M、适用D/H/P、原SQL、阶段和I/T/E，错误后SELECT 1恢复，未采精确错误文本/位置不填猜值。psql后续用-E抓取实际\d前后查询及版本，与上述人工探针分别保存，不以UI刷新成功代替字段/身份断言。

#### 命令行客户端内省流程

以下均为待新增/待运行的验收，无本轮客户端输出。两端各准备专用数据库 cli_catalog，owner 具有 CONNECT、public USAGE/CREATE 和目录读取权限；初始无 cli_probe，UTF8、空闲 I、search_path=public,pg_catalog。管理连接事务外建库，DDL 每条单独执行；清理 DROP TABLE IF EXISTS public.cli_probe 后由管理连接 DROP DATABASE cli_catalog，先断开其他连接。不能多语句隐式事务绕过第 5 章 DDL 门槛。

人工契约探针原 SQL（参数 []，文本结果；L/R/C 均不声称由 psql 生成）：

```sql
-- L
SELECT oid,datname FROM pg_catalog.pg_database WHERE datname='cli_catalog';
-- R
SELECT c.oid,n.nspname,c.relname,c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname='cli_probe';
-- C
SELECT a.attrelid,a.attnum,a.attname,a.atttypid,a.atttypmod,a.attnotnull FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname='cli_probe' AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum;
```

| ID/输入流程 | 目标行、描述、拒绝及所需产物 |
| --- | --- |
| CLI-LIBPQ-LIVE | 使用 PQexec 依次 L/R/C，初始 L 一行(D,cli_catalog)、R/C 零行但有完整目标描述；D 现场记录。`CREATE TABLE public.cli_probe (id integer NOT NULL, label varchar(7));` 后 R 一行(U,public,cli_probe,r)，C 两行(U,1,id,23,-1,true)、(U,2,label,1043,11,false)。`DROP TABLE public.cli_probe;` 后 R/C 零行，L 不变；重建同表获得新 U2≠U，字段不留旧值。逐列 PQfname/PQftype/PQfsize/PQfmod/PQftable/PQftablecol/PQfformat，PQntuples/PQgetisnull/PQgetvalue 与原始状态另存，NULL 不等于空字符串。类型 OID 按目标表，不沿用现有 text/25 近似。 |
| CLI-LIBPQ-PREPARED | 初始空表名时 PQprepare R、PQdescribePrepared，零参数 OID[]；建表后 PQexecPrepared（参数值/长度/格式[]，resultFormat=0），删表后再次执行。准备描述固定且行从零→一→零；C 同样执行。另用 `SELECT oid,relname FROM pg_catalog.pg_class WHERE oid=$1::oid;`，参数类型[26]，值[U]，分别文本十进制与四字节网络序 binary，结果文本；目标列 oid/26、name/19，一行(U,cli_probe)，NULL 参数零行，删表后旧 U 零行。新 portal 刷新与旧 portal 续读沿目录更新块，不能混用。 |
| CLI-PSQL-LIVE | 固定精确 psql 二进制，以 `psql --version` 记录版本，再对两端分别 `psql -X -E -v ON_ERROR_STOP=1 -d '<conninfo>' -f '<probe.sql>'`。占位 conninfo 由第 8 章注入实际地址/角色/库且脱敏；probe.sql 是待新增临时产物。按行写入 `\l cli_catalog`、`\dt public.cli_probe`，随后建表 SQL，再 `\dt public.cli_probe`、`\d public.cli_probe`，随后删表 SQL和再次同两条元命令；每条 SQL 单独一行。建表前/删表后无目标表；建表后列表一对象，描述有 id integer NOT NULL、label character varying(7)，不得出现虚构 owner/默认/约束。保存 -E 输出中的每条原 SQL、顺序、stdout/stderr、退出码及服务版本；无匹配表的客户端消息/退出策略以参考端实采，不能预造。 |
| CLI-EMPTY/PRIVILEGE | 真正空用户库仍运行 L/R/C 与 psql 列表/描述，系统行不能被误判成用户表。分别准备无库 CONNECT、可 CONNECT 但无 public USAGE、无表 SELECT 的非 owner 角色（角色/授权命令沿权限块夹具，待新增）。无 CONNECT 在连接阶段拒绝，目标 SQLSTATE 42501；有连接但对象使用受限时，目录行、无模式限定列表、显式描述与 `SELECT * FROM public.cli_probe;` 分别观察。无表 SELECT 不能抹掉可读目录事实，访问表拒绝目标 42501；具体元命令隐藏/显示与诊断阶段、SQLSTATE、消息待参考端采证，不把 intruder 启动失败测试当授权覆盖。 |
| CLI-RECOVERY | libpq 查询 `SELECT oid FROM pg_catalog.pg_missing;` 目标 42P01，随后 `SELECT 1;` 得一行1；扩展路径记录错误、Sync 和 I 状态。psql ON_ERROR_STOP 脚本拒绝后的退出码与完整错误另存，恢复用新连接 `SELECT 1;`，不要求已终止脚本继续执行。BEGIN 内恢复沿第 5/7 章，不新增事务实现。 |

第 8 章运行时必须把每条 psql 实采 SQL 的每个投影补齐首块字段映射，记录实际客户端版本、libpq 加载路径/版本、两端构建、命令/退出码和有效用例数。当前缺失这些产物是明确的后续取证项，不能将文档验收完成写成客户端功能通过。

#### 驱动元数据内省流程

以下全部为**待新增/待运行**验收；不是本轮测试结果。两端使用专用数据库 jdbc_catalog、UTF8、search_path=public,pg_catalog、空闲 I、autocommit=true；owner 有 CONNECT、schema USAGE/CREATE 和目录读取权限。锁定 REL42.7.8 对应 JAR，保存散列、getDriverVersion、Java 版本、服务完整构建与选项；prepareThreshold=1，hideUnprivilegedObjects=false，再独立连接以 true 重跑权限用例。实际 wire 参数类型、格式与后台版本分支须抓取，不能由 Java String 猜为 varchar/1043。

夹具每条单独执行，DDL 不能嵌入事务绕过第 5 章门槛：

```sql
CREATE TABLE public.jdbc_parent (b integer NOT NULL, a integer NOT NULL, CONSTRAINT jdbc_parent_pk PRIMARY KEY (b,a));
CREATE TABLE public.jdbc_probe (id integer PRIMARY KEY, b integer, a integer, label varchar(7) DEFAULT 'x', CONSTRAINT jdbc_probe_fk FOREIGN KEY (b,a) REFERENCES public.jdbc_parent (b,a) ON UPDATE CASCADE ON DELETE SET NULL);
CREATE INDEX jdbc_probe_label_idx ON public.jdbc_probe (label);
```

FK 若当前建表能力拒绝，记录建表阶段的真实错误；不能去掉 FK 后把 importedKeys 验收记为通过。先建父后子，清理先 DROP TABLE IF EXISTS public.jdbc_probe 再 jdbc_parent；管理连接待客户端断开后删专用库。注释/权限夹具沿相应块，均待新增。

完整调用输入（c 为上述连接，md=c.getMetaData()；返回 ResultSet 必须全部消费并关闭）：

```java
md.getSchemas("jdbc_catalog", "public");
md.getTables("jdbc_catalog", "public", "jdbc_probe", new String[]{"TABLE"});
md.getColumns("jdbc_catalog", "public", "jdbc_probe", "%");
md.getPrimaryKeys("jdbc_catalog", "public", "jdbc_parent");
md.getImportedKeys("jdbc_catalog", "public", "jdbc_probe");
md.getIndexInfo("jdbc_catalog", "public", "jdbc_probe", false, false);
md.getTypeInfo();
```

getSchemas 在关闭权限过滤、catalog 匹配时可由源码还原如下完整请求；? 是驱动 PreparedStatement 占位符，Java 参数位置 1 为 String public，不是抓取后的 $1/OID/格式记录：

```sql
SELECT nspname AS "TABLE_SCHEM", current_database() AS "TABLE_CATALOG" FROM pg_catalog.pg_namespace WHERE nspname <> 'pg_toast' AND (nspname !~ '^pg_temp_' OR nspname = (pg_catalog.current_schemas(true))[1]) AND (nspname !~ '^pg_toast_temp_' OR nspname = replace((pg_catalog.current_schemas(true))[1], 'pg_temp_', 'pg_toast_temp_')) AND nspname LIKE ? ORDER BY "TABLE_SCHEM"
```

其他六个调用的源码参数序列分别为 [public,jdbc_probe]、[public,jdbc_probe,%]、[public,jdbc_parent]、[public,jdbc_probe]、[public,jdbc_probe]、[]；types/unique 为生成 SQL 条件而非绑定值。ImportedKeys 的最大索引键数探针参数 []，类型缓存附加请求未采。driver getCatalog/启动请求也需纳入原始日志，不能只采主查询。

| ID/调用 | 行/字段与 NULL 验收、类型及变化 |
| --- | --- |
| JDBC-SCHEMAS/TABLES | schemas 应含 (public,jdbc_catalog)，tables 对夹具唯一 TABLE 行；校验两列/十列的名称顺序及所有投影，数据库/schema/名称、TABLE_TYPE、REMARKS、TYPE_CAT/TYPE_SCHEM/TYPE_NAME/SELF_REFERENCING_COL_NAME/REF_GENERATION 分别追溯上表。无注释 REMARKS=NULL；上述 SQL 的末五项是空字串。保存服务器 name/19 与驱动 ResultSet 描述，不能一律改成 text/25。 |
| JDBC-COLUMNS | 24 列按 tag Field 顺序逐项比较：TABLE_CAT/SCHEM/NAME、COLUMN_NAME、DATA_TYPE、TYPE_NAME、COLUMN_SIZE、BUFFER_LENGTH、DECIMAL_DIGITS、NUM_PREC_RADIX、NULLABLE、REMARKS、COLUMN_DEF、SQL_DATA_TYPE、SQL_DATETIME_SUB、CHAR_OCTET_LENGTH、ORDINAL_POSITION、IS_NULLABLE、SCOPE_CATALOG/SCHEMA/TABLE、SOURCE_DATA_TYPE、IS_AUTOINCREMENT、IS_GENERATEDCOLUMN。label 目标 JDBC VARCHAR=12、varchar、size=7、ordinal=4、nullable=1/YES、自增/生成均 NO，默认由真实表达式反解且两端规范比较；id 的 INTEGER=4、int4、ordinal=1、nullable=0/NO。BUFFER_LENGTH、未使用 SQL 类型字段、scope、无 domain 的 SOURCE_DATA_TYPE 为 NULL，REMARKS 无注释为 NULL；驱动本 tag CHAR_OCTET_LENGTH 复制 COLUMN_SIZE，不能自行换成编码字节数。合成描述 varchar/1043；DATA_TYPE/SOURCE_DATA_TYPE int2/21；其余计数等按 Field 为 int4/23，BUFFER_LENGTH/CHAR_OCTET_LENGTH 实为 varchar/1043。这些与服务器 attnum int8/20 分开验收。 |
| JDBC-PK/FK | PK 两行 b、a，KEY_SEQ=1、2，PK_NAME 用真实引用索引名，非按字母重排；六输出列均逐项核对。FK 十四列：两侧 CAT/SCHEM/NAME/COLUMN_NAME、KEY_SEQ、UPDATE_RULE、DELETE_RULE、FK_NAME、PK_NAME、DEFERRABILITY；两行 (b,b,1)、(a,a,2)，CASCADE=0、SET_NULL=2、NOT_DEFERRABLE=7，FK_NAME=jdbc_probe_fk。名称与索引名来自真实对象，不硬编码 AsterSQL 的 PRIMARY 代替参考端命名；差异须记录。服务器名称 name/19、KEY_SEQ/CASE int4/23，与 catalog 不匹配时驱动自造空结果中的 Field 类型分别取证。 |
| JDBC-INDEX | 十四列含标准十三列及本 tag 的 REMARKS：TABLE_CAT/SCHEM/NAME、NON_UNIQUE、INDEX_QUALIFIER、INDEX_NAME、TYPE、ORDINAL_POSITION、COLUMN_NAME、ASC_OR_DESC、CARDINALITY、PAGES、FILTER_CONDITION、REMARKS。应找到真实主索引及 jdbc_probe_label_idx，label 索引 NON_UNIQUE=true、位置1、COLUMN_NAME=label；unique=true 不含该非唯一索引，approximate=true 与 false 都取证。INDEX_QUALIFIER=NULL，普通无 predicate 的 FILTER_CONDITION=NULL、无注释 REMARKS=NULL；ASC_OR_DESC 只按真实 AM/option 判断。服务器名称 name/19、布尔16、TYPE/展开序号 int4/23、CARDINALITY float4/700、PAGES int4/23、定义文本25；不能照驱动 catalog 不匹配空结果的 INT8 Field 伪造 wire 类型。统计数值以参考现场和真实来源为准，不预设0。 |
| JDBC-TYPES | 18 合成列逐项采集：TYPE_NAME、DATA_TYPE、PRECISION、LITERAL_PREFIX/SUFFIX、CREATE_PARAMS、NULLABLE、CASE_SENSITIVE、SEARCHABLE、UNSIGNED_ATTRIBUTE、FIXED_PREC_SCALE、AUTO_INCREMENT、LOCAL_TYPE_NAME、MINIMUM_SCALE/MAXIMUM_SCALE、SQL_DATA_TYPE、SQL_DATETIME_SUB、NUM_PREC_RADIX。基础类型必须含 int4→INTEGER/4、varchar→VARCHAR/12，与规范类型表一致；serial 是驱动合成且无新增服务器 OID。合成文本 varchar/1043、标志 bool/16、DATA_TYPE/NULLABLE/SEARCHABLE/scale int2/21、precision/未用 SQL 类型字段/radix int4/23；CREATE_PARAMS/LOCAL_TYPE_NAME/未用 SQL 字段 NULL，引用符对不需引用类型 NULL。服务器仅 typname/name19、oid26 与补充查询字段另采，不能拿 JDBC DATA_TYPE 替代 t.oid。 |
| JDBC-PARAMETERS | 七项基线之外，各方法有 nullable 参数处分别测试 null、空字串、不同 catalog；getTypeInfo 无参数。pattern 参数 null 表示不加 LIKE，空字串不是 null；%/_ 是模式，转义取 getSearchStringEscape 并记录。PK/FK/index schema/table 是精确名称，含%不应被当通配符。types=null 与空数组、TABLE/VIEW、unique 两值、approximate 两值均采。catalog 不匹配可由驱动本地造零行，不得登记服务器空集；本 tag 的 getSchemas 空结果字段与非空路径存在差异，须保留实际描述而非补造统一顺序。 |
| JDBC-LIVE/CASE/PRIVILEGE | 建表前→建表后→ALTER TABLE public.jdbc_probe ADD COLUMN extra integer→删子表→重建的七项相关结果刷新；新元数据调用不能留旧 OID/列/约束。每步 autocommit，A/B 连接可见性及新 portal 沿目录更新块。带引号混合大小写及含下划线名称沿第 2/5 章，分别精确/模式查询。真实非 owner 的无 CONNECT、无 schema USAGE、无表 SELECT 夹具沿权限块；分别比较 hideUnprivilegedObjects 两值，不把无 SELECT 一概视为目录零行，不把 bootstrap root 测试当授权覆盖。 |
| JDBC-REJECTION/RECOVERY | 目标合法七调用均应成功；若必要目录/函数/表达式未实现，按目录求值块在分析/绑定或执行记录明确 0A000 能力拒绝、具体 SQL 与阶段，再 SELECT 1 验证恢复；真实缺关系用42P01，权限错误按42501契约。不能由本轮未运行推断实际 SQLSTATE 或错误文本，也不能吞错误返回空 ResultSet。 |

第 8 章后续实跑需保存每条原始 SQL/逐参数值类型格式、列描述与有效行数、getObject/getString/wasNull、getColumnType/getColumnTypeName、调用/异常栈及退出码，逐列回填首块模板（包含连接数据库/角色/path及列的表 OID/属性号）。无参考 PostgreSQL 18.6 构建、REL42.7.8 实际 JAR 或完整补充查询记录时，相应精确行为写未取证，不报告客户端通过。

### 后续细分

#### 目录执行链与证据模板

后续 25 块串行在上述五个三级标题下追加各自同名四级块，复用第 1 章身份及本模板；每个窄块只认领单一目录对象、同源字段组或一个客户端流程。先由 namespace 块补真实 schema/owner/ACL 来源及字段类型，再由对应对象块填关联键/定义/权限，求值和客户端块分别补有界拒绝与实际查询证据。本示例是后续验收输入，未新建或执行测试。

第 2/5 章交接名称/会话及 DDL 门禁，第 3 章交固定类型及 codec，第 4 章交正式函数身份，第 7 章补完整 wire、恢复和生命周期，第 8 章补真实客户端及 MySQL 自身基线。不得因为本块文档完成升级支持状态。正确性风险是伪造身份/来源或缓存旧事实；兼容性风险是 name→text、错误阶段及 schema/database 混同；性能未测。有界限制只记录当前事实，后续负载另立验证。Ready 文档检查采用任务指定结构、路径、前后契约及 diff 审阅；仓库验证技能缺失，无代码检查或功能实测结论。

#### 数据库目录映射

后续实施按字段来源拆窄块：先确认原生库可映射集合与稳定身份及 name codec，再补持久 owner/角色关联，最后按真实客户端需求补模板/连接策略；共享描述、安全关联及权限函数交各自块，psql 额外字段和真实版本查询交客户端块。本轮没有新建测试文件或实现任务，不修复这些缺口。

覆盖审阅：数据库名/OID、owner、模板、连接开关分别关联 DB-BASE/EMPTY/FRESH、DB-OWNER、DB-POLICY；六列 DATABASES_SQL 关联 DB-LIST，未知列和恢复关联 DB-REJECT。正确性风险是错误身份/关联或伪造 owner/策略，兼容性风险是 name→text、datdba→int8、TiDB database 与 PG schema 混同及将可枚举当可连接；性能未测。Ready 文档检查沿任务的五节/锚点、路径存在、前后契约、来源覆盖与 diff 审阅执行；验证技能缺失如前块所述，不运行 Cargo/Go、代码 lint 或真实客户端，不将文档完成升级为功能支持。

#### 命名空间目录映射

后续实施按窄块分别补：当前库 public/内建 namespace 的身份及 name codec；用户 schema 的真实持久来源和所属库键；owner/ACL 字段及角色关联（衔接角色权限块）；同名表 relnamespace 关联（衔接关系身份块）；目录新鲜度/跨连接隔离（衔接更新块）。普通名称解析及有效 search_path 继续遵守第 2/5 章，不能把本块列行能力当解析器已支持多 schema。

覆盖审阅：四字段逐项关联 NS-BASE/EMPTY、NS-OWNER、NS-ACL，OID 外键关联 NS-JOIN，元数据变化和同名 schema/跨库分别关联 NS-FRESH/DB；无权限 schema 保行与拒绝读表分别验。正确性风险是库/schema 层次或 OID 外键错误、伪造所有者/ACL；兼容性风险是 name/owner 类型不符、缺用户 schema 和误把路径过滤当目录过滤；性能未测。本轮仅文档，Ready 采用五节锚点、路径存在、来源覆盖、前后契约与 diff 审阅替代缺失的验证技能；不运行 Cargo/Go、代码 lint 或客户端，不以文档交付宣称功能已实现。

#### 关系身份与稳定标识

后续先独立补稳定身份/越界门槛与同库关系命名唯一性，再补 name/内部char及regclass路径往返、owner持久来源；按对应窄块接列/索引/序列外键与分区语义，不递归补齐子系统。覆盖映射：五个核心字段→REL-BASE/KEY/KIND/OWNER，持久原生ID/删除重建/双连接/重启→REL-LIFE，cast/未知数值/名称→REL-CAST/UNKNOWN/NAME，溢出→REL-RANGE；测试意图与目标验收不混为运行证据。

正确性风险是ID重用、非public状态查找、重复局部索引名、错误外键或伪造owner；兼容性风险是text替代name/内部char、regclass显示不随path、物理分区与PG关系混同；性能未测。Ready文档适用检查采用五节锚点、来源/路径、前后契约与diff人工审阅；`.agents/skills/tidb-verify-profile/SKILL.md`缺失，使用任务指定替代检查。不运行代码lint、Cargo/Go或差分客户端，不将文档完成升级为功能支持。下一步按批次5处理列目录与声明类型。

#### 列目录与声明类型

按最小事实组拆实施：①持久列ID到PG属性槽位及DROP/ADD/改名映射，先证明ATTR-LIFE；②规范声明OID/typmod、name与内部char的两格式描述及普通结果一致性，覆盖ATTR-BASE/TYPE；③真实NOT NULL、identity模式/序列关联和生成表达式分别实施，覆盖ATTR-NULL/IDENTITY/GENERATED。索引/系统/序列属性依各对象真实模型再开窄任务，不吞并完整catalog。

优先扩展现有独立pg_catalog_test.rs与pg_types_test.rs意图，不把Rust测试放进生产文件。每项后续实施需重新取源码/回归证据并遵循ExecPlan；本轮只交付本块文档，不代表功能已支持。pg_type/默认表达式/权限/元数据刷新及客户端采集沿后续批次接口，不能将其未实现反向阻塞本文档完成。

正确性风险为属性号重排、声明身份丢失与错误模式占位；兼容性风险为name/内部char描述、删除槽位和生成语义差异；性能未实测。使用Ready文档适用检查（五节/锚点、来源路径、累计依赖、前后契约、diff），因`.agents/skills/tidb-verify-profile/SKILL.md`缺失采用任务指定替代检查。未运行Cargo/Go、代码lint、真实客户端或18.6差分；下一批为内建类型目录映射。

#### 内建类型目录映射

后续按同源窄任务推进：①内建身份注册与已暴露OID闭环（含name/内部char、目录array/vector），实现TYPE-BASE/CLOSURE；②真实typelem/typarray/typdelim与vector区分，覆盖TYPE-ARRAY/JDBC；③format_type与同一声明typmod对齐、PG有效但未映射与不存在OID区分，覆盖TYPE-FORMAT/UNKNOWN；④域/复合身份、owner/ACL仅在各自真实模型具备后独立接线，未开放前覆盖TYPE-DOMAIN/ERROR拒绝。优先扩展现有独立pg_catalog_test.rs、pg_types_test.rs，不把测试写入生产文件，不因本块扩建整个类型系统。

正确性风险是固定表缺项、列/目录/结果身份分叉及把有效OID误作未知；兼容性风险是name描述、数组关联、NULL修饰符及默认ACL；性能未实测，当前固定类型查询仍遍历用户模型。Ready文档适用检查采用任务指定五节/锚点、引用路径、依赖/前后契约及diff审阅；`.agents/skills/tidb-verify-profile/SKILL.md`缺失。未运行Cargo/Go、代码lint、真实客户端或18.6差分；文档完成不表示类型功能实现完成，下一批为索引目录与定义反解。

#### 索引目录与定义反解

按事实来源拆窄实施任务：①持久表/局部索引身份、PKIsHandle不重复投影及schema命名，覆盖IDX-BASE/HANDLE/LIFE；②持久属性号与有序键、向量类型/下标，覆盖IDX-ORDER；③经方法等价证明的整定义、逐列及引用，覆盖IDX-DEF/QUOTE/UNKNOWN；④真实opclass/collation、状态和复杂形态门禁，分别覆盖IDX-AUX/STATE/EXTRA/SHAPE，缺依赖只列后续窄任务，不递归补完整索引系统。优先扩展现有独立pg_catalog_test.rs、pg_datagrip_test.rs，模板预期与客户端实采分开，覆盖IDX-TEMPLATE/ERROR。

本轮仅文档交付。正确性风险为虚拟索引重复、列槽位重排、身份关联错误及丢失条件的可执行定义；兼容性风险为命名冲突、text替代pg_node_tree、空opclass、排序/NULL策略和越界返回差异；性能未测，现有provider全库枚举与函数fallback扫描需后续取证。Ready文档检查采用任务指定五节/锚点、路径、依赖与前后契约、覆盖表和diff审阅；`.agents/skills/tidb-verify-profile/SKILL.md`缺失，使用明确替代检查。不运行Cargo/Go、代码lint、真实客户端或18.6差分，下一批为约束目录与定义反解。

#### 约束目录与定义反解

后续按事实组拆窄任务：①约束持久身份及普通唯一索引来源区分、虚拟主键去重，覆盖CON-BASE/IDENTITY；②有序属性关联、父支持索引选择与动作语义门槛，覆盖CON-ORDER/JOIN/ACTION；③CHECK规范表达式和保真反解、NULL/未知OID、名称刷新，覆盖CON-CHECK/DEF/UNKNOWN/LIFE/QUOTE；④真实执行/验证/延迟状态及辅助字段分别推进CON-STATE/VALIDATE/EXTRA。缺原生来源先记独立依赖，不吞并完整约束或操作符子系统。优先扩展现有独立pg_catalog_test.rs、pg_datagrip_test.rs，不把测试放生产文件。客户端模板与权限/隔离分别交CON-TEMPLATE/ACL/DB，沿后续专块采集。

正确性风险为复合FK配对错误、父索引关联错误、唯一索引伪装约束、伪造验证状态与丢失条件的反解；兼容性风险为name/text、pg_node_tree/text、xmin身份、零字节/空格及PG18新增约束字段；性能未测，当前全库provider及fallback扫描可被无关对象阻断。Ready文档适用检查采用任务指定五节锚点、路径、依赖/前后契约、字段→验收覆盖及diff审阅；`.agents/skills/tidb-verify-profile/SKILL.md`缺失，使用上述替代检查。未运行Cargo/Go、代码lint、真实客户端或18.6差分。本轮仅完成文档契约，下一批为默认表达式目录契约。

#### 默认表达式目录契约

按单一事实组拆分：默认显式存在性与独立对象身份、常量与编码的安全反解、二/三参数pg_get_expr及真实树codec、STORED/VIRTUAL生成表达式与列依赖、DDL后prepared刷新与权限投影。每项复用共享infoschema/model和现有目录工厂，在独立测试文件补齐上述验收；CURRENT_TIMESTAMP/序列运行语义分别交第4/5章，不扩大本任务实现。完成证据必须包含真实行/描述、DDL前后模型身份及固定PG参考构建；18.6、psql/DataGrip/JDBC/ORM和binary codec当前均未运行。

#### 视图定义内省契约

后续按单会话拆分：①持久ViewInfo到PG已绑定定义/Cols/CHECK OPTION的可靠来源与损坏拒绝；②OID/pretty/wrap与旧名称重载、schema安全反解和引号回归；③Parse后替换/重命名/跨连接目录更新；④固定psql/DataGrip流程及权限验证。分别复用关系、列、名称、函数注册和会话契约，pg_views/information_schema与物化视图定义接线单列缺口；本轮五节文档完成不表示功能实现完成。

#### 序列身份与依赖契约

按单一事实组另建实施任务（均待新增）：①真实序列声明类型与参数/pg_class一致性；②持久列拥有引用及a/i生命周期、attidentity、pg_get_serial_sequence局部接线；③SEQDEP-01–06的独立测试与客户端采集。先解决事实来源再开放能力，复用共享infoschema、OID和第5章分配链，不从SQL字符串或AUTO_INCREMENT补造所有权。文档完成只说明目标和验收已定义，不代表这些实现或测试已完成。

#### 函数目录与注册边界

后续独立实施按“签名事实与pg_type/namespace/OID关联及Describe类型”“目录白名单与普通SQL注册证据对照”“真实owner/ACL及权限/可见性”“客户端函数列表与反解采集”拆分，每块沿第4章共享注册链补独立回归，禁止用静态目录新增行替代普通SQL实现。用户函数生命周期与ACL夹具只在相应能力开放时执行，本块不提前建设完整子系统。

#### 对象注释与描述契约

按同源块另拆：①表/列/索引/序列注释映射与oid描述，联动稳定列槽位；②obj_description/col_description目录绑定和普通SQL工厂，复用真实来源；③数据库共享注释持久来源、shobj_description及跨库验证；④空串删除、Unicode、权限和元数据更新独立测试。每块先补待新增测试，记录真实客户端版本与SQL；角色及表空间分别设来源门槛，禁止递归扩建。本轮文档完成不表示功能完成。

#### 角色权限与所有权契约

后续按单一事实链拆分：①原生user/host到稳定PG角色身份及脱敏pg_user/pg_roles；②会话授权角色/有效角色与成员边；③单类对象持久owner及pg_get_userbyid；④单类ACL与对应has_*查询、grant option及授权失效。每项分别遵循第3/4/5章类型、正式注册、会话和第7章协议门禁，不扩成完整授权子系统。批次23–25补各客户端确切SQL/版本/参数；本块待新增验收不得登记为当前实测。

#### 可见性与名称遮蔽契约

后续窄任务依次补真实有效路径消费与三类可见性函数、regclass输入/显示一致性、目录截获与普通绑定分流、权限及两连接/DDL回归；扩充现有同目录独立测试，不将测试写进生产文件，不补完整子系统。需求映射：路径/关系身份→VIS-PATH/EMPTY；引号→QUOTE；OID显示/类型/参数→DISPLAY/ARG/TYPE/FUNC；授权独立→ACCESS；更新与隔离→LIFE/CONNECTION。

Ready按纯文档范围检查五节/锚点、路径存在、前后契约及diff；`.agents/skills/tidb-verify-profile/SKILL.md`缺失，采用任务指定替代检查，未执行其内容。正确性风险为目录误截获、错误OID和路径/角色快照混用；兼容性风险为引号合并、可见性与授权混淆、regclass显示或绑定时点差异；性能未测。本块完成只代表契约交付，所有新增验收及18.6/客户端运行仍待实施。

#### 标准模式表视图映射

后续分为：三个视图的方言路由与类型描述；共享模型到catalog/schema/关系字段及pg_class对照；真实角色过滤与owner定义脱敏；可更新/触发器/typed字段门槛；IS-*独立回归及固定客户端采集。每项实施另建计划，沿共享快照、OID及现有执行链；无来源字段明确拒绝，不递归补齐所有information_schema。当前仅完成文档契约，功能、性能及精确18.6行为仍待实施/实测。

#### 标准模式列映射

后续按单一事实来源拆为：PG标准列provider/域描述与列级权限；声明类型/长度精度及域数组映射；默认/identity/generated来源和反解；DDL后位置/名称/默认刷新与双入口客户端探针。每项复用关系、列、类型、默认、权限和第5章契约，仅为待新增实施输入；不得在本块递归补建上述子系统，也不把文档验收当作运行通过。

#### 标准模式约束映射

后续按事实组另拆实施：四视图方言路由/域描述；持久约束身份与pg_constraint/pg_index/真实依赖关联；复合键展开及父索引逆序位置；CHECK/PG18 NOT NULL依赖；逐视图角色过滤与刷新隔离。扩展现有独立测试，ISCON-*均待新增/待运行，不补建完整标准模式或客户端。

Ready文档适用检查使用五节锚点、全部本块路径存在性、前置提交/正文、字段→来源→目标→验收映射及diff审阅；`.agents/skills/tidb-verify-profile/SKILL.md`缺失，使用这些替代检查。正确性风险为同名约束误联、FK方向/序位错误与伪造父约束；兼容性风险为四种权限过滤、域wire描述及PG18 NOT NULL/enforced字段；性能未测，当前全库枚举不能推断可扩展性。本轮仅完成文档，不运行Cargo/Go、代码lint、真实客户端或18.6差分。下一批为批次19“无原生对象的响应策略”。

#### 无原生对象的响应策略

后续按ABS-SPACE/FDW/POLICY/TRIGGER/REWRITE/COLL/OP单独实施，先补固定字段/类型与绑定错误，再接真实事实和独立测试；默认对象/必需依赖不借用空集实现。验收ID覆盖策略表全部行，客户端模板需补确切版本/官方来源或实采，18.6需绑定构建。

Ready文档范围采用五节锚点、前置提交/正文、全部本块路径存在性、字段→来源→策略→验收及diff检查；`.agents/skills/tidb-verify-profile/SKILL.md`缺失，报告替代检查。正确性风险为把缺模型当无对象、NULL或误吞绑定错误；兼容性风险为默认对象缺失、零行描述错误及FK/视图内省缺口；性能未测。本轮只完成文档契约，不表示功能实现。建议下一批20继续有界目录求值契约。

#### 目录求值与普通查询边界

后续仅按必要探针拆为：分类/混合关系拒绝及遮蔽；JOIN与单次快照；CTE/子查询/有限数组类型；参数描述与codec；共享预算/取消及错误恢复。每项另建实施计划和独立回归，记录真实客户端完整SQL及版本后才扩白名单；通用SQL能力走共享内核。批次21接目录更新与连接隔离，本块只交路由/类型/资源边界，不吞并其实现。

#### 目录更新与连接隔离

后续分别拆为单次快照/provider屏障回归、DDL发布后R/C刷新、prepared结构失效与portal续读、跨库/连接上下文隔离，再单列事务型DDL开放后的提交/回滚目录观察。每项另建窄实施计划，优先复用上述独立测试，新增同步钩子明确标为待新增；不补建另一目录引擎。

Ready文档范围采用五节锚点、前置提交及正文、引用路径存在性、字段→来源→目标→验收和diff审阅；.agents/skills/tidb-verify-profile/SKILL.md缺失，采用替代检查。正确性风险为版本混合与旧身份复用，兼容性风险为name类型、列槽位及prepared错误阶段，性能/锁等待未测。本文完成仅表示契约交付，下一批22继续其指定主题。

#### 命令行客户端内省流程

按 CLI-LIBPQ 的行/描述及参数闭环、CLI-PSQL 的固定版本 -E 查询采集、CLI-EMPTY/PRIVILEGE 的空库与权限各拆独立实施/取证任务；测试及 probe.sql 均待新增。复用现有 pg_client_integration_test.rs 的真实 domain 接线并保持测试与生产文件分离，不在本任务扩写代码。第 7 章承接 wire 描述/错误恢复，第 8 章运行冻结查询、DDL 前后列表/描述/删表重查并保存证据。运行揭示的字段、类型或权限缺口回交对应窄块，不扩大本块为完整客户端兼容工程。

#### 驱动元数据内省流程

后续分别建立“schemas/tables 请求采证”“columns 与类型缓存请求”“复合主外键展开”“index 与真实统计”“typeInfo 与驱动合成字段”窄任务，每项复用本章公共字段契约，先固定 REL42.7.8 两端实采 SQL/参数与全部描述，再限定最小实现及独立测试。Java DatabaseMetaData 验收入口、JAR 散列、启动/清理、差分运行及证据归档交第 8 章；所有新测试/日志产物均待新增，不在本任务创建。

## 7. 线协议认证与数据通道

## 8. 客户端与双方言共存验收
