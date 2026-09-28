// Copyright 2026 AsterSQL.

//! 工作区根构建脚本。
//!
//! 这个文件承担两类职责：
//! 1. 为根级 harness 生成若干测试适配文件，让迁移中的子 crate 测试能在工作区根入口复用；
//! 2. 拉取 Go 生态里的 `kvproto`、`tipb` proto，生成并规范化 Rust 绑定。
//!
//! 设计重点不是“重新实现一套生成器”，而是尽量复用 Go 侧已经稳定的输入，
//! 让 Rust 构建期只做路径定位、轻量文本归一化和模块拼装，避免 Go/Rust 双方长期漂移。

use std::path::PathBuf;
use std::process::Command;

/// 解析指定 Go module 在本机缓存中的目录。
///
/// 正常情况下优先走 `go list -m`，因为它能处理 replace、vendor 或本地代理配置；
/// 如果当前环境没有可用的 `go` 命令，再回退到读取仓库 `go.mod` 和 `$HOME/go/pkg/mod`。
/// 这里宁可在路径不一致时直接失败，也不静默猜测目录，
/// 否则后续 proto 生成会在更远的位置以更难排查的方式出错。
fn go_module_dir(module: &str) -> PathBuf {
    // 第一优先级是复用 Go 工具链自己的模块解析结果，
    // 这样与仓库当前 `go env`、代理和 replace 语义保持一致。
    let output = Command::new("go")
        .args(["list", "-m", "-f", "{{.Dir}}", module])
        .output();
    if let Ok(output) = output {
        assert!(output.status.success(), "go list failed for {module}");
        return PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    }

    // 回退路径只覆盖“没有 go 可执行文件”这一类场景；
    // 版本号仍然从当前仓库 `go.mod` 读取，避免把 proto 锁到硬编码版本。
    let go_mod =
        std::fs::read_to_string("go.mod").expect("read go.mod for cached Go module fallback");
    let version = go_mod
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next() == Some(module))
                .then(|| fields.next())
                .flatten()
        })
        .unwrap_or_else(|| panic!("missing {module} version in go.mod"));
    let home = std::env::var("HOME").expect("HOME for cached Go module fallback");
    // 这里假设模块缓存使用 Go 默认布局，
    // 与上面 `go env GOMODCACHE` 的逻辑配合，尽量兼容轻量 CI 环境。
    let path = PathBuf::from(home)
        .join("go/pkg/mod")
        .join(format!("{module}@{version}"));
    assert!(
        path.is_dir(),
        "cached Go module is missing: {}",
        path.display()
    );
    path
}

/// 归一化上游 `.proto` 文件中的 `rustproto` 选项。
///
/// 仓库选择让生成结果使用更通用的运行时和字符串/字节表现形式，
/// 因此在真正调用 codegen 前先把几个与历史 Go 生态绑定较深的选项关掉。
/// 这样做的目的不是改 schema 语义，而是把生成器输出收敛到当前 Rust 工作区更稳定的形态。
fn normalized_proto(source: &std::path::Path, destination: &std::path::Path) {
    let source = std::fs::read_to_string(source).expect("read protobuf schema");
    // 这里只做文本级替换，不解析 protobuf AST，
    // 因为要改动的选项非常固定，保持简单反而更不容易引入新行为。
    std::fs::write(
        destination,
        source
            .replace(
                "option (rustproto.lite_runtime_all) = true;",
                "option (rustproto.lite_runtime_all) = false;",
            )
            .replace(
                "option (rustproto.carllerche_bytes_for_bytes) = true;",
                "option (rustproto.carllerche_bytes_for_bytes) = false;",
            )
            .replace(
                "option (rustproto.carllerche_bytes_for_string) = true;",
                "option (rustproto.carllerche_bytes_for_string) = false;",
            ),
    )
    .expect("write normalized protobuf schema");
}

/// 为根工作区生成“直接 include 即可编译”的测试适配文件。
///
/// 原始测试文件往往写给子 crate 自己使用，根 harness 里路径层级不同，
/// 所以这里移除 crate 级属性并把 crate 名改写为根入口下的可见路径。
/// 输出文件名按模块和测试场景命名，便于从根级测试清单定位来源。
fn root_test(out_dir: &std::path::Path, output: &str, source: &str, crate_name: &str) {
    // `#![...]` 这类 crate 级属性放到被 include 的上下文里通常会冲突，
    // 因此在适配阶段统一剥离，只保留测试主体。
    let test = std::fs::read_to_string(source)
        .unwrap_or_else(|err| panic!("read {source}: {err}"))
        .lines()
        .filter(|line| !line.trim_start().starts_with("#!["))
        .collect::<Vec<_>>()
        .join("\n")
        .replace(crate_name, &format!("crate::{crate_name}"));
    std::fs::write(out_dir.join(output), test).expect("write root test adapter");
    // Cargo 只要看到源测试有变化，就会重新生成适配文件，
    // 避免手工维护根目录镜像副本。
    println!("cargo:rerun-if-changed={source}");
}

/// 生成需要额外前导代码或批量文本替换的测试适配文件。
///
/// 与 `root_test` 相比，这里允许注入 `use`、`pub use` 或删除冲突片段，
/// 用来处理少量无法直接在根 harness 下重放的测试。
/// 仍然坚持“最小文本改写”策略，尽量不触碰测试本身的断言和流程。
fn adapted_test(
    out_dir: &std::path::Path,
    output: &str,
    source: &str,
    prelude: &str,
    replacements: &[(&str, &str)],
) {
    // 先和 `root_test` 一样清掉 crate 级属性，
    // 再按调用方给出的替换表做局部重写，保持每个任务的特殊规则显式可见。
    let mut test = std::fs::read_to_string(source)
        .unwrap_or_else(|err| panic!("read {source}: {err}"))
        .lines()
        .filter(|line| !line.trim_start().starts_with("#!["))
        .collect::<Vec<_>>()
        .join("\n");
    for (from, to) in replacements {
        test = test.replace(from, to);
    }
    std::fs::write(out_dir.join(output), format!("{prelude}\n{test}"))
        .expect("write adapted root test");
    // 适配文件完全由源文件派生，因此同样把变更监听挂回原文件。
    println!("cargo:rerun-if-changed={source}");
}

/// 根构建入口。
///
/// 执行顺序基本固定：
/// 1. 编译仓库自身的 gRPC proto；
/// 2. 生成迁移测试适配文件；
/// 3. 生成并规范化 `kvproto` / `tipb` 绑定。
/// 各阶段互相依赖较弱，但都复用同一个 `OUT_DIR` 作为构建期产物落点。
fn main() {
    // 这一组 proto 属于仓库自身源码，直接用 vendored `protoc` 生成，
    // 避免把系统安装的版本差异带进 CI。
    let external_workload = "pkg/extworkload/proto/externalworkload.proto";
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    unsafe { std::env::set_var("PROTOC", protoc) };
    tonic_build::configure()
        .build_server(true)
        .compile_protos(&[external_workload], &["pkg/extworkload/proto"])
        .expect("compile external workload proto");

    // 下面开始处理依赖于 Go 模块缓存的 proto。
    // 先解析 `GOMODCACHE`，失败时再退回默认的 `$HOME/go/pkg/mod`。
    let gomodcache = Command::new("go").args(["env", "GOMODCACHE"]).output();
    let gomodcache = match gomodcache {
        Ok(output) => {
            assert!(output.status.success(), "go env GOMODCACHE failed");
            PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
        }
        Err(_) => PathBuf::from(std::env::var("HOME").expect("HOME for GOMODCACHE fallback"))
            .join("go/pkg/mod"),
    };
    let kvproto = go_module_dir("github.com/pingcap/kvproto");
    let tipb = go_module_dir("github.com/pingcap/tipb");
    let gogo = gomodcache.join("github.com/gogo/protobuf@v1.3.2");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));

    // 这一批任务可以只做 crate 路径替换，因此统一走 `root_test`。
    // 167：`dxfmetric` 的测试主要依赖根级可见的 crate 名。
    // 这里不改断言，只把路径接回工作区根门面。
    // 169：`ingestmetric` 同样属于简单的根路径重写场景。
    // 迁移重点是确保指标常量和注册逻辑仍能被根 harness 访问。
    // 170：`lightning metric` 的迁移测试直接复用原文件最稳妥。
    // 生成适配副本比手写新测试更能保持和 Go 对照的一致性。
    // 171：`objectio` 测试主要验证对象存储层公开接口。
    // 根级适配只负责让原 crate 名在当前工作区重新可见。
    // 178：`timer metrics` 没有额外 include 或模块注入需求。
    // 因此保留原始测试主体即可覆盖计时器指标的迁移入口。
    // 179：`checksum` 的这批用例适合零改动迁入根 harness。
    // 通过场景后缀也便于和后面的补充适配区分。
    // 180：`column-mapping` 的 migration 测试只依赖 crate 路径修正。
    // 不需要像正式测试那样再注入额外 `use` 或替换表。
    // 181：`mathutil` 保持简单适配，避免构建脚本里混入数学逻辑假设。
    // 目标只是让根目录测试入口能继续跑到原始断言。
    // 182：`util/redact` 的 migration 测试同样属于路径改写型。
    // 真正的脱敏语义仍由源文件本身维护，这里不重新组织内容。
    // 184：`skip` 模块测试较轻量，直接镜像到根 harness 成本最低。
    // 保持任务号有助于和迁移记录一一对应。
    // 199：`lightning metric` 的正式测试也要接入根入口。
    // 它与 170 的 migration 测试分开生成，避免产物名冲突。
    // 204：`checksum` 正式测试与 migration 测试并存。
    // 分别生成独立适配文件，便于后续精确开关和定位失败来源。
    // 209：`redact` 正式测试放在这组简单适配的最后。
    // 它依旧只做 crate 名重写，不额外引入 prelude。
    for (output, source, crate_name) in [
        (
            "dxfmetric_migration_test.rs",
            "pkg/dxf/framework/dxfmetric/migration_aster_unit_test.rs",
            "astersql_dxf_framework_dxfmetric",
        ),
        (
            "ingestmetric_migration_test.rs",
            "pkg/ingestor/ingestmetric/migration_aster_unit_test.rs",
            "astersql_ingestor_ingestmetric",
        ),
        (
            "lightning_metric_migration_test.rs",
            "pkg/lightning/metric/migration_aster_unit_test.rs",
            "astersql_lightning_metric",
        ),
        (
            "objectio_migration_test.rs",
            "pkg/objstore/objectio/migration_aster_unit_test.rs",
            "astersql_objstore_objectio",
        ),
        (
            "timer_metrics_migration_test.rs",
            "pkg/timer/metrics/migration_aster_unit_test.rs",
            "astersql_timer_metrics",
        ),
        (
            "checksum_migration_test.rs",
            "pkg/util/checksum/migration_aster_unit_test.rs",
            "astersql_util_checksum",
        ),
        (
            "column_mapping_migration_test.rs",
            "pkg/util/column-mapping/migration_aster_unit_test.rs",
            "astersql_util_column_mapping",
        ),
        (
            "mathutil_migration_test.rs",
            "pkg/util/mathutil/migration_aster_unit_test.rs",
            "astersql_util_mathutil",
        ),
        (
            "redact_migration_test.rs",
            "pkg/util/redact/migration_aster_unit_test.rs",
            "astersql_util_redact",
        ),
        (
            "skip_migration_test.rs",
            "pkg/util/skip/migration_aster_unit_test.rs",
            "astersql_util_skip",
        ),
        (
            "lightning_metric_test.rs",
            "pkg/lightning/metric/metric_test.rs",
            "astersql_lightning_metric",
        ),
        (
            "checksum_test.rs",
            "pkg/util/checksum/checksum_test.rs",
            "astersql_util_checksum",
        ),
        (
            "redact_test.rs",
            "pkg/util/redact/redact_test.rs",
            "astersql_util_redact",
        ),
    ] {
        root_test(&out_dir, output, source, crate_name);
    }

    // 剩余任务需要额外 prelude 或定制替换：
    // 有的是补 `use` 以接回根路径，
    // 有的是删除子模块局部 `include!` / `mod` 声明，避免在根 harness 下重复定义。
    // 168：`engineapi` 适配前要先引入 `membuf`。
    // 同时把 `use crate::*;` 收窄成指向 `engineapi` 的根路径。
    // 176：`lockwaiter` 依赖更深的嵌套模块层级。
    // 这里额外注入 `config` 并删除 `use super::*;`，避免根级作用域失配。
    // 195：`time_vec` 生成测试原本依赖局部 `include!` 文件。
    // 在根 harness 下改成直接 `pub use` 公开模块，避免重复 include。
    // 197：`disk_sorter_1` 需要单独输出文件名以区别另一组 extsort 测试。
    // 内容本身无需额外改写，所以 replacement 表为空。
    // 205：`column_test` 不是 migration 文件，但需要在根入口补上模块导入。
    // 这样测试体里原本的裸符号仍能找到对应实现。
    // aes_layer：加密模块拆成多文件后，根路径需要显式指向根门面。
    // 这里先处理 `aes_layer`，后续同组文件复用相同的命名修正策略。
    // 206 aes：测试既依赖 `aes` 本体，也依赖 `aes_layer` 和 `crypt`。
    // prelude 一次性导出三者，避免在生成文件里散落多段补丁。
    // 206 crypt：与 `aes` 类似，同样需要三模块联动可见。
    // 保持 replacement 规则一致，可减少同一任务内部的路径漂移。
    // 206 main：`main_test` 不需要额外导入，但仍要单独生成文件。
    // 这样可以和同任务的其他测试共享产物目录而不互相覆盖。
    // 207 external：外部排序器测试与磁盘排序器测试拆成两个适配输出。
    // 分开生成能让失败日志直接映射回具体场景。
    // 393 json_impl：这里要把跨目录 `include!` 路径改成本仓库根下可解析的形式。
    // 同时把内部辅助函数提升到 `pub(super)`，让测试侧能继续访问。
    // 393 mydecimal_impl：`mydecimal` 只需把实现文件挂到根 harness。
    // 不再附加替换，避免影响大量数值逻辑细节。
    // 371 tiflash_impl：实现文件本身依赖 `vardef`。
    // 因此 prelude 先把它引进来，保持源码里原有引用可解析。
    // 371 migration_test：迁移测试自带的 `mod tiflash_replica_read` 会和根适配重复定义。
    // 这里显式删掉该声明，改为使用前面单独生成的实现适配文件。
    // 368 validator_impl：`validator` 实现里引用若干兄弟模块。
    // replacement 把这些 `crate::` 路径降到 `super::`，以适配根 harness 模块层级。
    // 207 disk：和 external sorter 同属 extsort，但覆盖的是磁盘路径分支。
    // 独立生成文件可避免两个测试场景混在一份适配代码里。
    // 211 sqlescape：只需要为工具函数测试补上模块导入。
    // 这样原测试里的自由函数调用不必逐个重写。
    // 212 table-router：除了导入 `router`，还需要额外暴露前面生成的 `selector`。
    // 这是因为该测试会直接依赖 trie selector 的实现细节。
    adapted_test(
        &out_dir,
        "engineapi_migration_test.rs",
        "pkg/ingestor/engineapi/migration_aster_unit_test.rs",
        "use crate::membuf;",
        &[("use crate::*;", "use crate::engineapi::*;")],
    );
    adapted_test(
        &out_dir,
        "lockwaiter_migration_test.rs",
        "pkg/store/mockstore/unistore/util/lockwaiter/migration_aster_unit_test.rs",
        "use crate::store::mockstore::unistore::util::lockwaiter::*;\nuse crate::config;",
        &[("use super::*;", "")],
    );
    adapted_test(
        &out_dir,
        "time_vec_test.rs",
        "pkg/expression/generator/time_vec_2_aster_unit_test.rs",
        "pub use crate::expression::generator::time_vec::*;",
        &[("include!(\"time_vec.rs\");", "")],
    );
    adapted_test(
        &out_dir,
        "disk_sorter_legacy_test.rs",
        "pkg/util/extsort/disk_sorter_1_aster_unit_test.rs",
        "",
        &[],
    );
    adapted_test(
        &out_dir,
        "column_mapping_test.rs",
        "pkg/util/column-mapping/column_test.rs",
        "use crate::util::column_mapping::*;",
        &[],
    );
    adapted_test(
        &out_dir,
        "encrypt_aes_layer_test.rs",
        "pkg/util/encrypt/aes_layer_test.rs",
        "",
        &[("encrypt::", "crate::util::encrypt::")],
    );
    adapted_test(
        &out_dir,
        "encrypt_aes_test.rs",
        "pkg/util/encrypt/aes_test.rs",
        "use crate::util::encrypt::{aes::*, aes_layer::*, crypt::*};",
        &[("use encrypt::", "use crate::util::encrypt::")],
    );
    adapted_test(
        &out_dir,
        "encrypt_crypt_test.rs",
        "pkg/util/encrypt/crypt_test.rs",
        "use crate::util::encrypt::{aes::*, aes_layer::*, crypt::*};",
        &[("use encrypt::", "use crate::util::encrypt::")],
    );
    adapted_test(
        &out_dir,
        "extsort_external_test.rs",
        "pkg/util/extsort/external_sorter_test.rs",
        "",
        &[],
    );
    adapted_test(
        &out_dir,
        "json_binary_impl.rs",
        "pkg/types/internal/json_binary/lib.rs",
        "",
        &[
            (
                "include!(concat!(\n    env!(\"CARGO_MANIFEST_DIR\"),\n    \"/../../../pkg/types/json_binary_functions.rs\"\n));",
                "include!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/pkg/types/json_binary_functions.rs\"));",
            ),
            ("fn quoteJSONString", "pub(super) fn quoteJSONString"),
        ],
    );
    adapted_test(
        &out_dir,
        "mydecimal_impl.rs",
        "pkg/types/mydecimal.rs",
        "",
        &[],
    );
    adapted_test(
        &out_dir,
        "tiflash_replica_read_impl.rs",
        "pkg/util/tiflash/tiflash_replica_read.rs",
        "use crate::vardef;",
        &[],
    );
    adapted_test(
        &out_dir,
        "tiflash_migration_test.rs",
        "pkg/util/tiflash/migration_aster_unit_test.rs",
        "",
        &[(
            concat!(
                "#",
                "[path = \"tiflash_replica_read.rs\"]\nmod tiflash_replica_read;"
            ),
            "",
        )],
    );
    adapted_test(
        &out_dir,
        "infoschema_validator_impl.rs",
        "pkg/infoschema/isvalidator/validator.rs",
        "",
        &[
            ("crate::logutil", "super::logutil"),
            ("crate::validatorapi", "super::validatorapi"),
            ("crate::vardef", "super::vardef"),
        ],
    );
    adapted_test(
        &out_dir,
        "extsort_disk_test.rs",
        "pkg/util/extsort/disk_sorter_test.rs",
        "",
        &[],
    );
    adapted_test(
        &out_dir,
        "sqlescape_test.rs",
        "pkg/util/sqlescape/utils_test.rs",
        "use crate::util::sqlescape::utils::*;",
        &[],
    );
    adapted_test(
        &out_dir,
        "table_router_test.rs",
        "pkg/util/table-router/router_test.rs",
        "use crate::util::table_router::router::*;\nuse crate::selector;",
        &[],
    );

    // `table-rule-selector` 的实现要被多个根级适配测试共享，
    // 因此单独抽成一个生成出来的 `selector.rs`，避免每个测试各自复制。
    let selector_source = "pkg/util/table-rule-selector/trie_selector.rs";
    let selector = std::fs::read_to_string(selector_source)
        .expect("read trie selector")
        .lines()
        .filter(|line| !line.starts_with("#!["))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(out_dir.join("selector.rs"), selector).expect("write selector module");
    println!("cargo:rerun-if-changed={selector_source}");

    // `kvproto` 绑定生成分三步：
    // 1. 把上游 proto 复制到临时目录并关掉不需要的 rustproto 选项；
    // 2. 用 `protobuf_codegen_pure` 生成 Rust 代码；
    // 3. 再做一轮输出归一化，去掉属性和命名差异。
    let kv_schemas = kvproto.join("proto");
    let kv_harness = out_dir.join("kvproto");
    std::fs::create_dir_all(&kv_harness).expect("create kvproto directory");
    // 这里只挑当前工作区真正需要的 kvproto 子集，
    // 既减少构建时间，也避免把未使用模块一并引入。
    let kv_names = [
        "brpb.proto",
        "coprocessor.proto",
        "deadlock.proto",
        "encryptionpb.proto",
        "errorpb.proto",
        "kvrpcpb.proto",
        "metapb.proto",
        "resource_manager.proto",
        "tracepb.proto",
    ];
    for name in kv_names {
        normalized_proto(&kv_schemas.join(name), &kv_harness.join(name));
    }
    let kv_inputs = kv_names
        .iter()
        .map(|name| kv_harness.join(name))
        .collect::<Vec<_>>();
    let kv_input_strings = kv_inputs
        .iter()
        .map(|p| p.to_str().unwrap())
        .collect::<Vec<_>>();
    let kv_harness_string = kv_harness.to_string_lossy().into_owned();
    let kv_schema_string = kv_schemas.to_string_lossy().into_owned();
    let kv_include = kvproto.join("include").to_string_lossy().into_owned();
    let gogo_string = gogo.to_string_lossy().into_owned();
    let out_string = out_dir.to_string_lossy().into_owned();
    // include 顺序显式保留 harness、原始 schema、kv include 与 gogo 依赖目录，
    // 这样生成器解析 import 时与 Go 模块布局一致。
    protobuf_codegen_pure::run(protobuf_codegen_pure::Args {
        out_dir: &out_string,
        includes: &[
            &kv_harness_string,
            &kv_schema_string,
            &kv_include,
            &gogo_string,
        ],
        input: &kv_input_strings,
        customize: Default::default(),
    })
    .expect("generate kvproto bindings");
    let brpb_path = out_dir.join("brpb.rs");
    // `brpb` 中几个 oneof 变体名称会落成不符合当前 Rust 命名习惯的标识，
    // 这里做一次后处理，把使用点和 enum 变体都统一成首字母大写形式。
    // 额外补 `pub type Gcs = GCS;` 是为了兼容上层仍可能引用旧名字的代码。
    let brpb = std::fs::read_to_string(&brpb_path)
        .expect("read brpb binding")
        .replace(
            "StorageBackend_oneof_backend::s3",
            "StorageBackend_oneof_backend::S3",
        )
        .replace(
            "StorageBackend_oneof_backend::gcs",
            "StorageBackend_oneof_backend::Gcs",
        )
        .replace(
            "StorageBackend_oneof_backend::azure_blob_storage",
            "StorageBackend_oneof_backend::AzureBlobStorage",
        )
        .replace("    s3(S3),", "    S3(S3),")
        .replace("    gcs(GCS),", "    Gcs(GCS),")
        .replace(
            "    azure_blob_storage(AzureBlobStorage),",
            "    AzureBlobStorage(AzureBlobStorage),",
        );
    std::fs::write(brpb_path, format!("{brpb}\npub type Gcs = GCS;\n"))
        .expect("normalize brpb oneof variants");
    // 生成器输出里的 crate 属性和模块文档在这里没有实际价值，
    // 统一剥离后，后续通过我们自己写出的 `mod.rs` 来组织模块边界。
    for name in kv_names {
        let path = out_dir
            .join(name.trim_end_matches(".proto"))
            .with_extension("rs");
        let generated = std::fs::read_to_string(&path).expect("read kvproto binding");
        let generated = generated
            .lines()
            .filter(|line| !line.starts_with("#!") && !line.starts_with("//!"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(path, generated).expect("normalize kvproto binding");
    }
    // 同时生成平铺版与嵌套版模块清单：
    // 前者给根构建直接 include，
    // 后者让临时 harness 目录自身也能作为一个模块树被引用。
    std::fs::write(
        out_dir.join("kvproto_bindings.rs"),
        kv_names
            .iter()
            .map(|name| name.trim_end_matches(".proto"))
            .map(|name| {
                format!(
                    "{}[path = {:?}] pub mod {name};\n",
                    '#',
                    out_dir.join(format!("{name}.rs"))
                )
            })
            .collect::<String>(),
    )
    .expect("write kvproto module list");
    std::fs::write(
        kv_harness.join("mod.rs"),
        kv_names
            .iter()
            .map(|name| name.trim_end_matches(".proto"))
            .map(|name| {
                format!(
                    "{}[path = {:?}] pub mod {name};\n",
                    '#',
                    out_dir.join(format!("{name}.rs"))
                )
            })
            .collect::<String>(),
    )
    .expect("write nested kvproto module list");

    // `tipb` 与 `kvproto` 处理方式基本一致。
    let tipb_proto = tipb.join("proto");
    let tipb_harness = out_dir.join("tipb");
    std::fs::create_dir_all(&tipb_harness).expect("create tipb directory");
    let tipb_names = ["schema.proto", "analyze.proto", "resourcetag.proto"];
    for name in tipb_names {
        normalized_proto(&tipb_proto.join(name), &tipb_harness.join(name));
    }
    let tipb_inputs = tipb_names
        .iter()
        .map(|name| tipb_harness.join(name))
        .collect::<Vec<_>>();
    let tipb_input_strings = tipb_inputs
        .iter()
        .map(|p| p.to_str().unwrap())
        .collect::<Vec<_>>();
    let tipb_harness_string = tipb_harness.to_string_lossy().into_owned();
    // `tipb` 依赖 `kvproto` 与 gogo include，
    // 因此这里沿用前面已经解析好的目录字符串，避免两套逻辑分叉。
    protobuf_codegen_pure::run(protobuf_codegen_pure::Args {
        out_dir: &out_string,
        includes: &["proto", &tipb_harness_string, &kv_include, &gogo_string],
        input: &tipb_input_strings,
        customize: Default::default(),
    })
    .expect("generate tipb bindings");
    for name in ["schema.rs", "analyze.rs", "resourcetag.rs"] {
        let path = out_dir.join(name);
        let generated = std::fs::read_to_string(&path).expect("read tipb binding");
        let generated = generated
            .lines()
            .filter(|line| !line.starts_with("#!") && !line.starts_with("//!"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(path, generated).expect("normalize tipb binding");
    }
    // `resourcetag` 是当前嵌套模块树里唯一需要直接 re-export 的入口，
    // 这里保持最小模块面，避免无关 proto 全部暴露出去。
    std::fs::write(
        tipb_harness.join("mod.rs"),
        format!(
            "{}[path = {:?}] pub mod resourcetag;\npub use resourcetag::*;\n",
            '#',
            out_dir.join("resourcetag.rs")
        ),
    )
    .expect("write nested tipb module list");

    // 把仓库内直接读取的 proto 文件挂到 Cargo 依赖追踪里。
    println!("cargo:rerun-if-changed={external_workload}");
}
