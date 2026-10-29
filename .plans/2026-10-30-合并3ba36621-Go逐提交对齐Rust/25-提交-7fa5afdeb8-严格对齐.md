# 任务 25: 提交 7fa5afdeb8 严格对齐 Rust

批次：【批次 25】；依赖批次：批次 24（顺序与共享资源依赖，不把 Git 父子顺序视为业务依赖）

状态：未开始

目的：Go 1.27.3 升级：区分格式化和实际语义，核查溢出诊断、region ID 十进制错误、生成链及工具依赖。

来源：Go commit `7fa5afdeb8412ec26fd848054a9bc1f703a2eb90`；父提交 `da76b94491e935de3d9fed575b574123c988b682`；标题：build: upgrade Go to 1.27.3 (#72180)

预计会话范围：用户要求一来源 commit 一任务，共 34 个来源路径；大提交在本任务内按行为分段，跨上下文维护阶段证据，不拆来源或吞并其他提交。

## 提交对齐范围硬约束

只负责本来源 diff、不可缺少的最小局部接线和原测试意图。先核对现有 Rust，等价实现只取证；每项生产修改必须映射到本任务具体 Go hunk 或不可缺少的局部接线依据。禁止递归补建完整子系统、通用基础设施、其他 commit；无关缺口仅列范围外建议，不阻塞已被证明的本增量。不得用桩、空样例、编译通过、零测试或删减逻辑代替完成。保留条件顺序、错误、数据形状、状态、事务、并发、副作用和原测试分支。后续替代必须记录完整 SHA、源码去向与保留行为，不恢复已撤销逻辑。

保留 PostgreSQL 的 SqlDialect::PostgreSQL、$n 参数、Parse/Bind/Execute/Sync、portal 生命周期、SQLSTATE、OID/二进制编码、search_path、事务失败状态和类型语义。Go 的 MySQL 专用规则只在既有方言边界内对齐，不能覆盖 PostgreSQL 的 NULL 顺序、强制转换、标识符或系统变量逻辑。

外部 Rust 依赖在独立上游仓库移植、提交并打 tag；本仓库只统一引用已发布 Git tag，不得 vendor/third_party/复制源码/[patch] 本地覆盖。清理本任务临时副本。Rust 测试放独立文件；保留 PingCAP Apache License，真正可用后添加 // Copyright 2026 AsterSQL.。不得覆盖、暂存或回滚其他未提交工作，尤其 PostgreSQL 当前功能任务。

## 文件

Git 核验的全部来源变更路径（R 行保留旧/新路径，含构建、生成、文档及测试）：

    M	.github/actions/tidb_build/action.yml
    M	.github/workflows/bazel-build-crossbuild.yml
    M	.github/workflows/bazel-lint-crossbuild.yml
    M	.github/workflows/check-bazel-prepare.yml
    M	.github/workflows/generate-bazel-files.yml
    M	DEPS.bzl
    M	Dockerfile
    M	Dockerfile.enterprise
    M	WORKSPACE
    M	br/cmd/br/main_test.go
    M	br/pkg/restore/ingestrec/foreign_key.go
    M	build/image/base
    M	build/image/parser_test
    M	build/linter/gofmt/BUILD.bazel
    M	build/linter/gofmt/analyzer.go
    A	build/linter/gofmt/analyzer_test.go
    D	build/patches/org_golang_x_tools.patch
    M	docs/tidb_http_api.md
    M	go.mod
    M	go.sum
    M	pkg/executor/main_test.go
    M	pkg/executor/show.go
    M	pkg/expression/builtin_encryption_test.go
    M	pkg/parser/go.mod
    M	pkg/parser/parser.go
    M	pkg/parser/parser.y
    M	pkg/server/handler/tikv_handler.go
    M	pkg/server/internal/packetio/packetio_test.go
    M	pkg/util/hack/map_abi_go127.go
    M	tests/integrationtest/r/executor/charset.result
    M	tests/integrationtest/r/expression/builtin.result
    M	tests/integrationtest/r/new_character_set_builtin.result
    M	tests/realtikvtest/startertest/docker-compose/Dockerfile.test
    M	tests/realtikvtest/startertest/docker-compose/README.md

Rust 候选不等于必须修改；缺失路径明确为执行期计划新增：

- `br/cmd/br/main_test.rs`：排除，现有。依据：br/cmd/br/main_test.go；新增 port_7fa5afdeb8_ 前缀的本提交行为回归；优先扩展已有测试。
- `br/pkg/restore/ingestrec/foreign_key_test.rs`：排除，现有独立测试，优先扩展。依据：新增 port_7fa5afdeb8_ 前缀的本提交行为回归；优先扩展已有测试。
- `br/pkg/restore/ingestrec/foreign_key.rs`：不变，现有。依据：br/pkg/restore/ingestrec/foreign_key.go。
- `br/pkg/restore/ingestrec/lib.rs`：排除，现有。依据：仅在新增模块或测试未挂载时做必要装配；现有测试已挂载则不变，AST 根含业务语义需同步说明。
- `pkg/executor/main_test.rs`：排除，现有。依据：pkg/executor/main_test.go；新增 port_7fa5afdeb8_ 前缀的本提交行为回归；优先扩展已有测试。
- `pkg/executor/show.rs`：不变，现有。依据：pkg/executor/show.go。
- `pkg/expression/builtin_encryption_test.rs`：排除，现有。依据：pkg/expression/builtin_encryption_test.go；新增 port_7fa5afdeb8_ 前缀的本提交行为回归；优先扩展已有测试。
- `pkg/parser/parser_test.rs`：排除，现有独立测试，优先扩展。依据：新增 port_7fa5afdeb8_ 前缀的本提交行为回归；优先扩展已有测试。
- `pkg/parser/lib.rs`：M（含业务逻辑，修改才同步说明），现有。依据：仅在新增模块或测试未挂载时做必要装配；现有测试已挂载则不变，AST 根含业务语义需同步说明。
- `pkg/parser/parser.rs`：M，现有。依据：pkg/parser/parser.go。
- `pkg/server/handler/tikv_handler_test.rs`：排除，现有独立测试，优先扩展。依据：新增 port_7fa5afdeb8_ 前缀的本提交行为回归；优先扩展已有测试。
- `pkg/server/handler/lib.rs`：排除，现有。依据：仅在新增模块或测试未挂载时做必要装配；现有测试已挂载则不变，AST 根含业务语义需同步说明。
- `pkg/server/handler/tikv_handler.rs`：M，现有。依据：pkg/server/handler/tikv_handler.go。
- `pkg/util/hack/map_abi_test.rs`：排除，现有独立测试，优先扩展。依据：新增 port_7fa5afdeb8_ 前缀的本提交行为回归；优先扩展已有测试。
- `pkg/util/hack/lib.rs`：排除，现有。依据：仅在新增模块或测试未挂载时做必要装配；现有测试已挂载则不变，AST 根含业务语义需同步说明。
- `pkg/util/hack/map_abi.rs`：M，现有。依据：pkg/util/hack/map_abi_go127.go。

Cargo 所有者（只在证明必要时改依赖）：

- `br/cmd/br/Cargo.toml`，package `astersql-br-cmd-br`，已核实存在。
- `br/pkg/restore/ingestrec/Cargo.toml`，package `astersql-br-pkg-restore-ingestrec`，已核实存在。
- `pkg/executor/Cargo.toml`，package `astersql-executor`，已核实存在。
- `pkg/expression/Cargo.toml`，package `astersql-expression`，已核实存在。
- `pkg/parser/Cargo.toml`，package `astersql-parser`，已核实存在。
- `pkg/server/handler/Cargo.toml`，package `astersql-server-handler`，已核实存在。
- `pkg/util/hack/Cargo.toml`，package `astersql-util-hack`，已核实存在。
- `Cargo.toml`、`Cargo.lock` 为共享元数据候选；`docs/docs-manifest.md` 仅按下列净变化维护。

## 上下文

Go 1.27.3 升级：区分格式化和实际语义，核查溢出诊断、region ID 十进制错误、生成链及工具依赖。 按串行批次保护共享写入、格式化/lint 和提交窗口，不因 Cargo 分槽而并行。

## Go 变更定位清单

所有 hunk 都需在覆盖记录中有结论；行号属于本来源快照。新增函数列出补充入口，不以文件名替代行为阅读。

### `.github/actions/tidb_build/action.yml`

    @@ -7 +7 @@ inputs:

### `.github/workflows/bazel-build-crossbuild.yml`

    @@ -32 +32 @@ jobs:

### `.github/workflows/bazel-lint-crossbuild.yml`

    @@ -28 +28 @@ jobs:

### `.github/workflows/check-bazel-prepare.yml`

    @@ -28 +28 @@ jobs:

### `.github/workflows/generate-bazel-files.yml`

    @@ -35 +35 @@ jobs:

### `DEPS.bzl`

    @@ -11,2 +11,2 @@ def go_deps():
    @@ -18,2 +18,2 @@ def go_deps():
    @@ -25,2 +25,2 @@ def go_deps():
    @@ -32,2 +32,2 @@ def go_deps():
    @@ -46,2 +46,2 @@ def go_deps():
    @@ -53,2 +53,2 @@ def go_deps():
    @@ -109,2 +109,2 @@ def go_deps():
    @@ -116,2 +116,2 @@ def go_deps():
    @@ -319,2 +319,2 @@ def go_deps():
    @@ -347,2 +347,2 @@ def go_deps():
    @@ -354,2 +354,2 @@ def go_deps():
    @@ -445,2 +445,2 @@ def go_deps():
    @@ -459,2 +459,2 @@ def go_deps():
    @@ -623,7 +622,0 @@ def go_deps():
    @@ -795,2 +788,2 @@ def go_deps():
    @@ -837,2 +830,2 @@ def go_deps():
    @@ -844,2 +837,2 @@ def go_deps():
    @@ -914,2 +907,2 @@ def go_deps():
    @@ -918 +911 @@ def go_deps():
    @@ -920,3 +913,3 @@ def go_deps():
    @@ -928,2 +921,2 @@ def go_deps():
    @@ -932 +925 @@ def go_deps():
    @@ -934,3 +927,3 @@ def go_deps():
    @@ -939 +932 @@ def go_deps():
    @@ -941,3 +934,10 @@ def go_deps():
    @@ -1007,0 +1008,7 @@ def go_deps():
    @@ -1014,0 +1022,7 @@ def go_deps():
    @@ -1341,2 +1355,9 @@ def go_deps():
    @@ -1383,2 +1404,2 @@ def go_deps():
    @@ -1474,2 +1495,2 @@ def go_deps():
    @@ -1509,2 +1530,2 @@ def go_deps():
    @@ -1537,2 +1558,2 @@ def go_deps():
    @@ -1579,2 +1600,2 @@ def go_deps():
    @@ -1607,2 +1628,2 @@ def go_deps():
    @@ -1958,2 +1979,2 @@ def go_deps():
    @@ -1965,2 +1986,2 @@ def go_deps():
    @@ -2051,0 +2073,7 @@ def go_deps():
    @@ -2056,2 +2084,2 @@ def go_deps():
    @@ -2070,2 +2098,2 @@ def go_deps():
    @@ -2077,2 +2105,2 @@ def go_deps():
    @@ -2121,0 +2150,7 @@ def go_deps():
    @@ -2530,2 +2565,2 @@ def go_deps():
    @@ -2698,2 +2733,2 @@ def go_deps():
    @@ -2912,2 +2947,2 @@ def go_deps():
    @@ -3041,7 +3075,0 @@ def go_deps():
    @@ -3059,2 +3087,2 @@ def go_deps():
    @@ -3080,2 +3108,2 @@ def go_deps():
    @@ -3164,2 +3192,2 @@ def go_deps():
    @@ -3227,2 +3255,2 @@ def go_deps():
    @@ -3262,2 +3290,2 @@ def go_deps():
    @@ -3283,2 +3311,2 @@ def go_deps():
    @@ -3290,2 +3318,2 @@ def go_deps():
    @@ -3297,2 +3325,2 @@ def go_deps():
    @@ -3325,2 +3353,2 @@ def go_deps():
    @@ -3455 +3483 @@ def go_deps():
    @@ -3457,3 +3485,3 @@ def go_deps():
    @@ -3535,2 +3563,2 @@ def go_deps():
    @@ -3556,2 +3584,2 @@ def go_deps():
    @@ -3703,2 +3731,2 @@ def go_deps():
    @@ -4004,2 +4032,2 @@ def go_deps():
    @@ -4039,2 +4067,2 @@ def go_deps():
    @@ -4076,0 +4105,7 @@ def go_deps():
    @@ -4081,2 +4116,2 @@ def go_deps():
    @@ -4102,2 +4137,2 @@ def go_deps():
    @@ -4144,2 +4179,2 @@ def go_deps():
    @@ -4186,2 +4221,2 @@ def go_deps():
    @@ -4235,2 +4270,2 @@ def go_deps():
    @@ -4270,2 +4305,2 @@ def go_deps():
    @@ -4277,2 +4312,2 @@ def go_deps():
    @@ -4375,2 +4410,2 @@ def go_deps():
    @@ -4382,2 +4417,2 @@ def go_deps():
    @@ -4480,2 +4515,2 @@ def go_deps():
    @@ -4530,2 +4565,2 @@ def go_deps():
    @@ -4538,2 +4573,2 @@ def go_deps():
    @@ -4545,2 +4580,2 @@ def go_deps():
    @@ -4552,2 +4587,2 @@ def go_deps():
    @@ -4657,2 +4692,2 @@ def go_deps():
    @@ -4755,2 +4790,2 @@ def go_deps():
    @@ -5906,2 +5941,2 @@ def go_deps():
    @@ -5920,2 +5955,2 @@ def go_deps():
    @@ -6251,0 +6287,7 @@ def go_deps():
    @@ -6263,2 +6305,2 @@ def go_deps():
    @@ -6291,2 +6333,2 @@ def go_deps():
    @@ -6382,2 +6424,2 @@ def go_deps():
    @@ -6389,2 +6431,2 @@ def go_deps():
    @@ -6403,2 +6445,2 @@ def go_deps():
    @@ -6431,2 +6473,2 @@ def go_deps():
    @@ -6438,2 +6480,2 @@ def go_deps():
    @@ -6459,2 +6501,2 @@ def go_deps():
    @@ -6466,2 +6508,2 @@ def go_deps():
    @@ -6473,2 +6515,2 @@ def go_deps():
    @@ -6480,2 +6522,2 @@ def go_deps():
    @@ -6487,2 +6529,2 @@ def go_deps():
    @@ -6501,6 +6543,2 @@ def go_deps():
    @@ -6738,0 +6777,7 @@ def go_deps():
    @@ -6743,2 +6788,2 @@ def go_deps():

### `Dockerfile`

    @@ -19 +19 @@

### `Dockerfile.enterprise`

    @@ -17 +17 @@

### `WORKSPACE`

    @@ -87,2 +87,2 @@ load("//:DEPS.bzl", "go_deps")
    @@ -99 +99 @@ go_download_sdk(

### `br/cmd/br/main_test.go`

    @@ -57,0 +58 @@ func TestMain(m *testing.M) {

### `br/pkg/restore/ingestrec/foreign_key.go`

    @@ -46,8 +46,8 @@ func newForeignKeyRecordKey(

### `build/image/base`

    @@ -33 +33 @@ RUN --mount=type=cache,target=/var/cache/dnf \

### `build/image/parser_test`

    @@ -17 +17 @@ FROM rockylinux:9

### `build/linter/gofmt/BUILD.bazel`

    @@ -1 +1 @@
    @@ -10 +10,2 @@ go_library(
    @@ -13,0 +15,9 @@ go_library(

### `build/linter/gofmt/analyzer.go`

    @@ -18,0 +19,2 @@ import (
    @@ -21 +23 @@ import (
    @@ -22,0 +25 @@ import (
    @@ -53 +56,8 @@ func run(pass *analysis.Pass) (any, error) {
    @@ -57,2 +67,3 @@ func run(pass *analysis.Pass) (any, error) {
    @@ -64 +75 @@ func run(pass *analysis.Pass) (any, error) {

### `build/linter/gofmt/analyzer_test.go`

    @@ -0,0 +1,72 @@

### `build/patches/org_golang_x_tools.patch`

    @@ -1,262 +0,0 @@

### `docs/tidb_http_api.md`

    @@ -57,0 +58,2 @@ listener entirely. Normal-mode HTTP API availability is unchanged.

### `go.mod`

    @@ -3 +3 @@ module github.com/pingcap/tidb
    @@ -20 +20 @@ require (
    @@ -36 +36 @@ require (
    @@ -53 +53,2 @@ require (
    @@ -63,2 +64,2 @@ require (
    @@ -76 +77,2 @@ require (
    @@ -82,0 +85 @@ require (
    @@ -84 +87 @@ require (
    @@ -89 +92,2 @@ require (
    @@ -106,0 +111 @@ require (
    @@ -112,0 +118 @@ require (
    @@ -121 +127 @@ require (
    @@ -124,3 +130,4 @@ require (
    @@ -145 +152 @@ require (
    @@ -147,4 +154,4 @@ require (
    @@ -152 +159 @@ require (
    @@ -154,0 +162 @@ require (
    @@ -158 +166 @@ require (
    @@ -165,0 +174 @@ require (
    @@ -167,0 +177,2 @@ require (
    @@ -169 +180,2 @@ require (
    @@ -170,0 +183,9 @@ require (
    @@ -179,0 +201 @@ require (
    @@ -193,47 +214,0 @@ require (
    @@ -243,0 +219,4 @@ require (
    @@ -245,0 +225 @@ require (
    @@ -249 +228,0 @@ require (
    @@ -253 +231,0 @@ require (
    @@ -254,0 +233 @@ require (
    @@ -265,0 +245,3 @@ require (
    @@ -285 +266,0 @@ require (
    @@ -286,0 +268,2 @@ require (
    @@ -288 +270,0 @@ require (
    @@ -289,0 +272 @@ require (
    @@ -292,0 +276 @@ require (
    @@ -299,4 +283,4 @@ require (
    @@ -305,0 +290,2 @@ require (
    @@ -308,0 +295 @@ require (
    @@ -311 +297,0 @@ require (
    @@ -313 +298,0 @@ require (
    @@ -315,0 +301 @@ require (
    @@ -317 +302,0 @@ require (
    @@ -319,0 +305 @@ require (
    @@ -325 +311 @@ require (
    @@ -327,2 +313,7 @@ require (
    @@ -334,0 +326 @@ require (
    @@ -338,0 +331 @@ require (
    @@ -348,3 +341,5 @@ require (
    @@ -354 +349 @@ require (
    @@ -355,0 +351 @@ require (

### `go.sum`

    @@ -1304,2 +1304,2 @@ cloud.google.com/go/workflows v1.12.4/go.mod h1:yQ7HUqOkdJK4duVtMeBCAOPiN1ZF1E9p
    @@ -1346,2 +1346,2 @@ github.com/Masterminds/semver/v3 v3.1.1/go.mod h1:VPu/7SZ7ePZ3QOrcuXROw5FAcLl4a0
    @@ -1444,2 +1444,2 @@ github.com/armon/consul-api v0.0.0-20180202201655-eb2c6b5be1b6/go.mod h1:grANhF5
    @@ -1514,2 +1514,2 @@ github.com/boombuler/barcode v1.0.1/go.mod h1:paBWMcWSl3LHKBqUq+rly7CNSldXjb2rDl
    @@ -1661,2 +1661,2 @@ github.com/fatih/color v1.15.0/go.mod h1:0h5ZqXfHYED7Bhv2ZJamyIOUej9KtShiJESRwBD
    @@ -1675,2 +1675,2 @@ github.com/fsnotify/fsnotify v1.4.7/go.mod h1:jwhsz4b93w/PPRr/qN1Yymfu8t87LnFCMo
    @@ -1789,4 +1789,4 @@ github.com/golang/snappy v1.0.0/go.mod h1:/XxbfmMg8lxefKM7IXC3fBNl/7bRcc72aCRzEW
    @@ -1947,2 +1947,2 @@ github.com/hashicorp/go-version v1.2.1/go.mod h1:fltr4n8CU8Ke44wwGCBoEymUuxUHl09
    @@ -2014,2 +2014,2 @@ github.com/kisielk/errcheck v1.5.0/go.mod h1:pFxgyoBC7bSaBwPgfKdkLd5X25qrDl4LWUI
    @@ -2074,2 +2074,2 @@ github.com/mattn/go-colorable v0.1.13/go.mod h1:7S9/ev0klgBDR4GtXTXX8a3vIGJpMovk
    @@ -2081,2 +2081,2 @@ github.com/mattn/go-isatty v0.0.19/go.mod h1:W+V8PltTTMOvKvAeJH7IuucS94S2C6jfK/D
    @@ -2086,2 +2086,2 @@ github.com/mattn/go-runewidth v0.0.13/go.mod h1:Jdepj2loyihRzMpdS35Xk/zdY8IAYHsh
    @@ -2092,2 +2092,2 @@ github.com/matttproud/golang_protobuf_extensions v1.0.1/go.mod h1:D8He9yQNgCq6Z5
    @@ -2248,2 +2248,2 @@ github.com/rogpeppe/go-internal v1.9.0/go.mod h1:WtVeX8xhTBvf0smdhujwtBcq4Qrzq/f
    @@ -2283,2 +2283,2 @@ github.com/sirupsen/logrus v1.8.1/go.mod h1:yWOB1SBYBC5VeMP7gHvWumXLIWorT60ONWic
    @@ -2328 +2327,0 @@ github.com/stretchr/objx v0.5.0/go.mod h1:Yh+to48EsGEfYuaHDzXPcE3xhTkx73EhmCGUpE
    @@ -2329,0 +2329,2 @@ github.com/stretchr/objx v0.5.2/go.mod h1:FRsXN1f5AsAjCGJKqEizvkpNtU+EGNCLh3NxZ/
    @@ -2343 +2343,0 @@ github.com/stretchr/testify v1.8.4/go.mod h1:sz/lmYIOXD/1dqDmKjjqLyZ2RngseejIcXl
    @@ -2344,0 +2345,2 @@ github.com/stretchr/testify v1.11.1/go.mod h1:wZwfW3scLgRK+23gO65QZefKpKQRnfz6sD
    @@ -2371,6 +2373,6 @@ github.com/tidwall/sjson v1.2.5/go.mod h1:Fvgq9kS/6ociJEDnK0Fk1cpYF4FIW6ZF7LAe+6
    @@ -2531,0 +2534,2 @@ go.yaml.in/yaml/v3 v3.0.4/go.mod h1:DhzuOOF2ATzADvBadXxruRBLzYTpT36CKvDb3+aBEFg=
    @@ -2565,2 +2569,2 @@ golang.org/x/crypto v0.24.0/go.mod h1:Z1PMYSOR5nyMcyAVAIQSKCDwalqy85Aqn1x3Ws4L5D
    @@ -2582,2 +2586,2 @@ golang.org/x/exp v0.0.0-20260112195511-716be5621a96/go.mod h1:nzimsREAkjBCIEFtHi
    @@ -2632,2 +2636,2 @@ golang.org/x/mod v0.17.0/go.mod h1:hTbmBsO62+eylJbnUtE2MGJUyE7QWk4xUqPFrRgJ+7c=
    @@ -2703,2 +2707,2 @@ golang.org/x/net v0.26.0/go.mod h1:5YKkiSynbBIh3p6iOc/vibscux0x38BZDkn8sCUPxHE=
    @@ -2765,2 +2769,2 @@ golang.org/x/sync v0.7.0/go.mod h1:Czt+wKu1gCyEFDUtn0jG5QVvpJ6rzVqr5aXyt9drQfk=
    @@ -2867,2 +2871,2 @@ golang.org/x/sys v0.21.0/go.mod h1:/VUhepiaJMQUp4+oa/7Zr1D23ma6VTLIYjOOTFZPUcA=
    @@ -2892,2 +2896,2 @@ golang.org/x/term v0.21.0/go.mod h1:ooXLefLobQVslOqselCNF4SxFAaoS6KujMbsGzSDmX0=
    @@ -2917,2 +2921,2 @@ golang.org/x/text v0.16.0/go.mod h1:GhwF1Be+LQoKShO3cGOHzqOgRrGaYc9AvblQOmPVHnI=
    @@ -2993,2 +2997,2 @@ golang.org/x/tools v0.21.1-0.20240508182429-e35e4ccd0d2d/go.mod h1:aiJjzUbINMkxb
    @@ -3393,2 +3397,2 @@ google.golang.org/protobuf v1.32.0/go.mod h1:c6P6GXX6sHbq/GpV6MGZEdwhWPcYBgnhAHh
    @@ -3439,2 +3443,2 @@ honnef.co/go/tools v0.1.3/go.mod h1:NgwopIslSNH47DimFoV78dnkksY2EFtX0ajyb3K/las=

### `pkg/executor/main_test.go`

    @@ -63,0 +64 @@ func TestMain(m *testing.M) {

### `pkg/executor/show.go`

    @@ -2933 +2933 @@ func (e *ShowExec) fetchShowImportJobs(ctx context.Context) error {
    @@ -2937 +2937 @@ func tryFillViewColumnType(ctx context.Context, sctx sessionctx.Context, is info

### `pkg/expression/builtin_encryption_test.go`

    @@ -592 +592 @@ func TestCompress(t *testing.T) {
    @@ -595,4 +595,4 @@ func TestCompress(t *testing.T) {
    @@ -622,3 +622,4 @@ func TestUncompress(t *testing.T) {

### `pkg/parser/go.mod`

    @@ -3 +3 @@ module github.com/pingcap/tidb/pkg/parser

### `pkg/parser/parser.go`

    @@ -19138 +19138 @@ yynewstate:

### `pkg/parser/parser.y`

    @@ -6999 +6999 @@ Int64Num:

### `pkg/server/handler/tikv_handler.go`

    @@ -276 +276 @@ func (t *TikvHandlerTool) GetRegionsMeta(regionIDs []uint64) ([]RegionMeta, erro

### `pkg/server/internal/packetio/packetio_test.go`

    @@ -82,7 +82,18 @@ func TestPacketIOWriteCompressed(t *testing.T) {
    @@ -272,5 +283,4 @@ func TestCompressedWriterLong(t *testing.T) {
    @@ -279 +288,0 @@ func TestCompressedWriterLong(t *testing.T) {
    @@ -286 +295,3 @@ func TestCompressedWriterLong(t *testing.T) {

### `pkg/util/hack/map_abi_go127.go`

    @@ -315 +315 @@ const (

### `tests/integrationtest/r/executor/charset.result`

    @@ -224,2 +224,2 @@ compress(a)

### `tests/integrationtest/r/expression/builtin.result`

    @@ -3126,2 +3126,2 @@ HEX(COMPRESS(a))
    @@ -3133,2 +3133,2 @@ a	HEX(b)

### `tests/integrationtest/r/new_character_set_builtin.result`

    @@ -269 +269 @@ md5(compress(a))	md5(compress(b))	md5(compress(c))
    @@ -273 +273 @@ md5(compress(a))	md5(compress(b))	md5(compress(c))

### `tests/realtikvtest/startertest/docker-compose/Dockerfile.test`

    @@ -1 +1 @@

### `tests/realtikvtest/startertest/docker-compose/README.md`

    @@ -49 +49 @@ The default external images in `docker-compose.yml`:

新增/修改声明入口：`TestFormattingDiagnostics`、`cap`、`tryFillViewColumnType`。

## Rust 说明文档同步清单

| Rust 源文件 | 动作 | 同目录说明动作 | manifest 动作 | 分类依据 |
| --- | --- | --- | --- | --- |
| br/cmd/br/main_test.rs | 排除 | 不创建/不更新 | 不变 | 独立测试 |
| br/pkg/restore/ingestrec/foreign_key_test.rs | 排除 | 不创建/不更新 | 不变 | 独立测试 |
| br/pkg/restore/ingestrec/foreign_key.rs | 不变 | 不变 | 不变 | 参考或纯 Go 格式调整，不制造文档 diff |
| br/pkg/restore/ingestrec/lib.rs | 排除 | 不创建/不更新 | 不变 | 仅模块导出/测试挂载的骨架 |
| pkg/executor/main_test.rs | 排除 | 不创建/不更新 | 不变 | 独立测试 |
| pkg/executor/show.rs | 不变 | 不变 | 不变 | 参考或纯 Go 格式调整，不制造文档 diff |
| pkg/expression/builtin_encryption_test.rs | 排除 | 不创建/不更新 | 不变 | 独立测试 |
| pkg/parser/parser_test.rs | 排除 | 不创建/不更新 | 不变 | 独立测试 |
| pkg/parser/lib.rs | M | A（缺失说明新增） `pkg/parser/lib.rs.md` | 新增唯一链接，净 +1 | 根文件含实际业务逻辑；若只复用已挂载测试无需修改，则源/说明均不变 |
| pkg/parser/parser.rs | M | M `pkg/parser/parser.rs.md` | 已有条目保持分组/排序/勾选，净 0 | 实际业务逻辑，仅实际改动时同步；等价则记不变 |
| pkg/server/handler/tikv_handler_test.rs | 排除 | 不创建/不更新 | 不变 | 独立测试 |
| pkg/server/handler/lib.rs | 排除 | 不创建/不更新 | 不变 | 仅模块导出/测试挂载的骨架 |
| pkg/server/handler/tikv_handler.rs | M | M `pkg/server/handler/tikv_handler.rs.md` | 已有条目保持分组/排序/勾选，净 0 | 实际业务逻辑，仅实际改动时同步；等价则记不变 |
| pkg/util/hack/map_abi_test.rs | 排除 | 不创建/不更新 | 不变 | 独立测试 |
| pkg/util/hack/lib.rs | 排除 | 不创建/不更新 | 不变 | 仅模块导出/测试挂载的骨架 |
| pkg/util/hack/map_abi.rs | M | M `pkg/util/hack/map_abi.rs.md` | 已有条目保持分组/排序/勾选，净 0 | 实际业务逻辑，仅实际改动时同步；等价则记不变 |

每个候选恰好一行；实际不变改记不变且不制造说明 diff。已有 manifest 条目但说明缺失只补说明不加计数；说明已存在但无条目只补唯一条目。若本提交确需 D/R，先补具体旧/新路径及 hunk 依据：D 同删说明/链接并净减一，R 同步说明/链接并保留勾选和总数；不扩大到其他来源。章节至少包含：文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南、验证依据；明确已接线、仅测试可见及 Go 独有。

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

## 测试计划

Go 1.27.3 升级：区分格式化和实际语义，核查溢出诊断、region ID 十进制错误、生成链及工具依赖。

原 Go 测试入口（必须逐个读输入、输出、错误与副作用，保留全部原测试意图）：

- `br/cmd/br/main_test.go`：`TestMain`。
- `build/linter/gofmt/analyzer_test.go`：`TestFormattingDiagnostics`。
- `pkg/executor/main_test.go`：`TestMain`。
- `pkg/expression/builtin_encryption_test.go`：`TestCompress`、`TestUncompress`。
- `pkg/server/internal/packetio/packetio_test.go`：`TestCompressedWriterLong`、`TestPacketIOWriteCompressed`。
- SQL 契约 `tests/integrationtest/r/executor/charset.result`：来源变化的输入与预期输出逐项映射真实 Rust 执行入口。
- SQL 契约 `tests/integrationtest/r/expression/builtin.result`：来源变化的输入与预期输出逐项映射真实 Rust 执行入口。
- SQL 契约 `tests/integrationtest/r/new_character_set_builtin.result`：来源变化的输入与预期输出逐项映射真实 Rust 执行入口。

独立 Rust 测试安排（`port_7fa5afdeb8_` 是明确计划新增前缀，未建测试时不能以零测试通过）：

- `br/cmd/br/main_test.rs`：扩展该文件，新增 `port_7fa5afdeb8_` 前缀用例覆盖 `astersql-br-cmd-br` 所有本提交生产差异；已有等价测试可复用并将其精确名称/命令写入覆盖记录，不为无生产影响 crate 强造逻辑。确认测试挂入 `br/cmd/br/Cargo.toml` 的 lib 测试入口。
- `br/pkg/restore/ingestrec/foreign_key_test.rs`：扩展该文件，新增 `port_7fa5afdeb8_` 前缀用例覆盖 `astersql-br-pkg-restore-ingestrec` 所有本提交生产差异；已有等价测试可复用并将其精确名称/命令写入覆盖记录，不为无生产影响 crate 强造逻辑。确认测试挂入 `br/pkg/restore/ingestrec/Cargo.toml` 的 lib 测试入口。
- `pkg/executor/main_test.rs`：扩展该文件，新增 `port_7fa5afdeb8_` 前缀用例覆盖 `astersql-executor` 所有本提交生产差异；已有等价测试可复用并将其精确名称/命令写入覆盖记录，不为无生产影响 crate 强造逻辑。确认测试挂入 `pkg/executor/Cargo.toml` 的 lib 测试入口。
- `pkg/expression/builtin_encryption_test.rs`：扩展该文件，新增 `port_7fa5afdeb8_` 前缀用例覆盖 `astersql-expression` 所有本提交生产差异；已有等价测试可复用并将其精确名称/命令写入覆盖记录，不为无生产影响 crate 强造逻辑。确认测试挂入 `pkg/expression/Cargo.toml` 的 lib 测试入口。
- `pkg/parser/parser_test.rs`：扩展该文件，新增 `port_7fa5afdeb8_` 前缀用例覆盖 `astersql-parser` 所有本提交生产差异；已有等价测试可复用并将其精确名称/命令写入覆盖记录，不为无生产影响 crate 强造逻辑。确认测试挂入 `pkg/parser/Cargo.toml` 的 lib 测试入口。
- `pkg/server/handler/tikv_handler_test.rs`：扩展该文件，新增 `port_7fa5afdeb8_` 前缀用例覆盖 `astersql-server-handler` 所有本提交生产差异；已有等价测试可复用并将其精确名称/命令写入覆盖记录，不为无生产影响 crate 强造逻辑。确认测试挂入 `pkg/server/handler/Cargo.toml` 的 lib 测试入口。
- `pkg/util/hack/map_abi_test.rs`：扩展该文件，新增 `port_7fa5afdeb8_` 前缀用例覆盖 `astersql-util-hack` 所有本提交生产差异；已有等价测试可复用并将其精确名称/命令写入覆盖记录，不为无生产影响 crate 强造逻辑。确认测试挂入 `pkg/util/hack/Cargo.toml` 的 lib 测试入口。

红灯：先写调用真实入口的独立用例，用下列同一命令取得修复前行为断言失败；新增 API 可先最小接线，编译失败不算红灯。绿灯：修复后重跑，所有断言通过且有效测试数大于 0。已等价时取当前绿灯与源码证据，不人为回退。只 mock 外部 PD/S3/网络/时钟边界，真实序列化、事务、回滚不能换成桩。

## 步骤

1. 读 plan.md、本任务、当前仓库规则和最近 Rust 实现/测试，核对来源命令、未提交修改和 PostgreSQL 基线。
2. 对每个 hunk 标注已有等价/需修复/后续替代/Go 专用，并建立独立回归的红灯或当前绿灯。
3. 仅实现本增量及必要局部接线；每项生产修改填写具体 Go diff 依据，保留 PostgreSQL 分支。
4. Rust 改动先 cargo fmt --all，再聚焦验证/Ready，逐项同步合格说明及 manifest 净变化。
5. 自审差异和覆盖记录；满足门槛后删除本任务文件并独立提交。中断保留证据，清理本任务锁和进程。

## 验证

命令从仓库根执行。确切来源命令：

    git show -s --format=fuller 7fa5afdeb8412ec26fd848054a9bc1f703a2eb90
    git diff --name-status -M da76b94491e935de3d9fed575b574123c988b682 7fa5afdeb8412ec26fd848054a9bc1f703a2eb90
    git diff --no-ext-diff --unified=40 da76b94491e935de3d9fed575b574123c988b682 7fa5afdeb8412ec26fd848054a9bc1f703a2eb90

取得槽位后，计划新增前缀的聚焦红/绿命令（若复用现有等价测试，在本任务记精确替代过滤条件与证据；不得全包/零测试替代）：

    cargo test --manifest-path br/cmd/br/Cargo.toml -p astersql-br-cmd-br --lib port_7fa5afdeb8_ -- --test-threads=1
    cargo test --manifest-path br/pkg/restore/ingestrec/Cargo.toml -p astersql-br-pkg-restore-ingestrec --lib port_7fa5afdeb8_ -- --test-threads=1
    cargo test --manifest-path pkg/executor/Cargo.toml -p astersql-executor --lib port_7fa5afdeb8_ -- --test-threads=1
    cargo test --manifest-path pkg/expression/Cargo.toml -p astersql-expression --lib port_7fa5afdeb8_ -- --test-threads=1
    cargo test --manifest-path pkg/parser/Cargo.toml -p astersql-parser --lib port_7fa5afdeb8_ -- --test-threads=1
    cargo test --manifest-path pkg/server/handler/Cargo.toml -p astersql-server-handler --lib port_7fa5afdeb8_ -- --test-threads=1
    cargo test --manifest-path pkg/util/hack/Cargo.toml -p astersql-util-hack --lib port_7fa5afdeb8_ -- --test-threads=1

代码 Ready：

    cargo fmt --all
    cargo fmt --all -- --check
    make lint
    git diff --check

生产接线/依赖变化再验证实际 NextGen 二进制（不代替行为测试）：

    cargo build --manifest-path cmd/tidb-server/Cargo.toml -p astersql-cmd-tidb-server --bin astersql-cmd-tidb-server --features nextgen

PostgreSQL 保护：相关共享路径修改前后运行并比较基线；纯 Go 栈格式/注释且 Rust 不变可以明确记录不适用。不能借已有失败隐藏新回归。

    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_query_test:: -- --test-threads=1
    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_extended_test:: -- --test-threads=1
    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_transaction_test:: -- --test-threads=1
    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_types_test:: -- --test-threads=1
    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_session_test:: -- --test-threads=1
    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_client_integration_test::pg_core_libpq_insert_readback -- --test-threads=1

客户端依赖 libpq 和 Python，缺失/失败如实报告；不得 ignored 或协议替身代替。类型/时间/函数变化另补相应现有 pg_* 测试边界，不混入其他任务未提交功能。

Go 对照按 docs/agents/testing-flow.md 决定 failpoint、记录和 RealTiKV。执行期新增/移动 Go、改 import/测试/依赖/Bazel 元数据或出现工具链错误，构建前先 make bazel_prepare。failpoint 必须启用并清理；集成记录用 run-tests.sh -r；RealTiKV 后台 playground，成功/失败/中断清理进程和本任务数据。禁止默认 make bazel_lint_changed。

文档检查：逐个实际 A/M/R 说明执行 test -f，验证上述全部章节；核查 docs/docs-manifest.md 的本任务链接存在、条目唯一、分组排序和勾选未扰动、总数按净量变化。D/R 还要确认旧说明/旧链接不残留。范围外缺失不补齐；文档验证不替代行为验证。

成功信号：检查退出码 0、相关测试数 > 0、无新失败；所有 hunk 有实际入口/断言或有据不适用结论；接线变化通过 NextGen 编译；文档同步一致。报告槽位、绝对 CARGO_TARGET_DIR、命令/退出码/数量、未验证项，以及正确性/兼容性/性能风险。


文档精确校验命令（只验证本任务实际修改的候选；不为范围外既有缺失扩展工作）：

    python3 - <<'DOC_CHECK'
    from pathlib import Path
    import subprocess, re
    candidates = ['pkg/parser/lib.rs', 'pkg/parser/parser.rs', 'pkg/server/handler/tikv_handler.rs', 'pkg/util/hack/map_abi.rs']
    modified = set(subprocess.check_output(['git','diff','--name-only','HEAD'], text=True).splitlines())
    modified.update(subprocess.check_output(['git','ls-files','--others','--exclude-standard'], text=True).splitlines())
    index = Path('docs/docs-manifest.md').read_text()
    old = subprocess.check_output(['git','show','HEAD:docs/docs-manifest.md'], text=True)
    links = re.findall(r'\]\(([^)]+)\)', index)
    oldlinks = re.findall(r'\]\(([^)]+)\)', old)
    checked = 0
    for source in candidates:
        if source not in modified:
            continue
        doc = Path(source + '.md')
        link = '../' + str(doc)
        if not Path(source).exists():
            assert not doc.exists() and link not in links, source
            continue
        assert doc.is_file(), doc
        body = doc.read_text()
        for heading in ['文件定位','核心职责','主要符号','执行流程','数据与状态','依赖与调用关系','错误处理与边界','并发与资源生命周期','与 Go 版本的对应关系','扩展指南','验证依据']:
            assert re.search(r'^##+ .*' + re.escape(heading), body, re.M), (doc, heading)
        assert links.count(link) == 1, (doc, '清单链接不唯一或缺失')
        checked += 1
    before = int(re.search(r'计划为 (\d+) 个', old)[1])
    after = int(re.search(r'计划为 (\d+) 个', index)[1])
    oldset = {x for x in oldlinks if x.endswith('.rs.md')}
    newset = {x for x in links if x.endswith('.rs.md')}
    assert after - before == len(newset - oldset) - len(oldset - newset), '清单计数净变化不一致'
    print('本任务已检查说明:', checked, '；无源码变化时 0 仅表示不适用，不是行为测试证据')
    DOC_CHECK

同时人工审查 `git diff -- docs/docs-manifest.md` 的分组排序、原勾选状态，以及实际重命名的旧路径清理；若执行时新增候选，先更新上面的具体候选列表。

## 完成

真实行为测试、逐项覆盖证据、适用 Ready、PostgreSQL 保护和 Rust 说明同步齐全后才标记 `已完成`。Rust 已等价时取得当前绿灯，不人为回退。Go 既有回归失败按 AGENTS.md 记录后可不阻塞；此豁免不适用于 Rust 行为未证明、零测试或未接线。范围外待办不继续阻塞已对齐的本提交。

完成后必须删除本编号任务文件，将本任务代码、独立测试、.rs.md、docs/docs-manifest.md、必要 manifest/lock 与任务文件删除整理成一个独立 Git commit。使用 ./skills/git-commit/SKILL.md，逐路径暂存，执行 git diff --cached --check 和 git diff --cached 核对没有其他任务或用户修改，再提交；Git commit 是必须收尾。即使无需 Rust 修改，也提交任务文件删除。删除前将覆盖证据摘要放入提交正文以保留记录。不修改只读 plan.md，不自行 push、amend 或 rebase。阻塞则在本任务记录 `已阻塞` 与具体原因，不提交、不删除。

## Go 到 Rust 覆盖记录

按每个 hunk 补齐以下记录；同一路径多个 hunk 要逐个展开，不以初始未开始冒充证据。

| Go 行为/差异入口 | Rust 真实入口候选 | 已有/修复/后续完整 SHA | 输入输出、命令、退出码、数量 |
| --- | --- | --- | --- |
| `.github/actions/tidb_build/action.yml`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `.github/workflows/bazel-build-crossbuild.yml`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `.github/workflows/bazel-lint-crossbuild.yml`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `.github/workflows/check-bazel-prepare.yml`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `.github/workflows/generate-bazel-files.yml`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `DEPS.bzl`（96 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `Dockerfile`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `Dockerfile.enterprise`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `WORKSPACE`（2 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `br/cmd/br/main_test.go`（1 个 hunk） | `br/cmd/br/main_test.rs` | 未开始，逐项取证 | 未开始，不得当成功 |
| `br/pkg/restore/ingestrec/foreign_key.go`（1 个 hunk） | `br/pkg/restore/ingestrec/foreign_key.rs` | 未开始，逐项取证 | 未开始，不得当成功 |
| `build/image/base`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `build/image/parser_test`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `build/linter/gofmt/BUILD.bazel`（3 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `build/linter/gofmt/analyzer.go`（6 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `build/linter/gofmt/analyzer_test.go`（1 个 hunk） | 原测试意图映射上述独立回归 | 未开始，逐项取证 | 未开始，不得当成功 |
| `build/patches/org_golang_x_tools.patch`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `docs/tidb_http_api.md`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `go.mod`（50 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `go.sum`（32 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `pkg/executor/main_test.go`（1 个 hunk） | `pkg/executor/main_test.rs` | 未开始，逐项取证 | 未开始，不得当成功 |
| `pkg/executor/show.go`（2 个 hunk） | `pkg/executor/show.rs` | 未开始，逐项取证 | 未开始，不得当成功 |
| `pkg/expression/builtin_encryption_test.go`（3 个 hunk） | `pkg/expression/builtin_encryption_test.rs` | 未开始，逐项取证 | 未开始，不得当成功 |
| `pkg/parser/go.mod`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `pkg/parser/parser.go`（1 个 hunk） | `pkg/parser/parser.rs` | 未开始，逐项取证 | 未开始，不得当成功 |
| `pkg/parser/parser.y`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `pkg/server/handler/tikv_handler.go`（1 个 hunk） | `pkg/server/handler/tikv_handler.rs` | 未开始，逐项取证 | 未开始，不得当成功 |
| `pkg/server/internal/packetio/packetio_test.go`（4 个 hunk） | 原测试意图映射上述独立回归 | 未开始，逐项取证 | 未开始，不得当成功 |
| `pkg/util/hack/map_abi_go127.go`（1 个 hunk） | `pkg/util/hack/map_abi.rs` | 未开始，逐项取证 | 未开始，不得当成功 |
| `tests/integrationtest/r/executor/charset.result`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `tests/integrationtest/r/expression/builtin.result`（2 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `tests/integrationtest/r/new_character_set_builtin.result`（2 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `tests/realtikvtest/startertest/docker-compose/Dockerfile.test`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |
| `tests/realtikvtest/startertest/docker-compose/README.md`（1 个 hunk） | 构建/生成/资料/SQL 影响核查，无影响须证据 | 未开始，逐项取证 | 未开始，不得当成功 |

## 范围外建议

不规划本提交以外的实现。发现历史缺口/其他提交/通用基础设施问题，只写具体路径、证据与建议归属；不扩写、不擅自回滚既有工作。

## Progress（进度）

- [ ] 核对来源、已有实现与 PostgreSQL 基线。
- [ ] 补齐每个增量的红绿/等价/替代覆盖。
- [ ] 完成适用 Ready、文档同步与自审。
- [ ] 达标后删除编号文件并独立 Git commit。

## Surprises & Discoveries（发现）

规划未运行行为测试；缺失候选是计划新增，不代表已实现或已接线。Ready skill 路径缺失和 Bazel 不可用需执行时复查。

## Decision Log（决策）

2026-10-30：一来源一任务，严格局部对齐；串行保护共享源码、说明、manifest、fmt/lint 和 Git index；独立回归按来源前缀聚焦，不能减少原测试分支。

## Outcomes & Retrospective（结果）

未开始执行，尚无 Rust 对齐结论。完成前记录实际结果、证据、风险与未验证项，摘要写入独立提交正文后删除本编号文件。
