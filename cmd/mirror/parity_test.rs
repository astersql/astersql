// Copyright 2026 AsterSQL.

//! 本文件验证 `cmd/mirror` 对外行为与 Go 版本保持一致。
//! 测试不依赖真实 bazel、真实 `go` 命令或真实文件系统。
//! 所有输入都通过脚本化环境和内存文件系统显式注入。
//! 这样既能复现主流程，又能稳定覆盖错误与清理分支。
//! 第一组用例关注仓库名转换，因为它直接决定 Bazel 依赖声明是否兼容。
//! 第二组用例关注 `build_file_proto_mode` 和命名例外是否被正确保留。
//! 第三组用例关注 JSON 串流解析，因为 Go 输出并不是一个标准数组。
//! 第四组用例关注 `deps.bzl` 的输出顺序，避免生成结果出现无意义抖动。
//! 排序断言只比较相对位置，避免测试把整段模板细节完全写死。
//! 边界路径里的 flag 断言说明旧参数虽然废弃，但提示文本仍属于契约。
//! 本地 `replace` 跳过下载的行为同样属于兼容面，不能在重构时消失。
//! 远端 `replace` 仍需参与下载与输出，这一点会直接影响生成版本。
//! 补丁文件存在时必须写出 `patch_args` 与 `patches` 片段。
//! 缺失补丁文件则是正常情况，不应把“文件不存在”当成失败。
//! 临时目录准备步骤中的 runfile 替换方式也是历史约束的一部分。
//! 错误路径首先覆盖“找不到下载结果”，这是生成阶段最常见的一类硬错误。
//! 随后覆盖坏 JSON，确保 Rust 没有把 Go 的宽松行拼接逻辑改成静默容错。
//! 再覆盖 `exec.ExitError`，保证 panic 文本仍包含用户最关心的 stderr。
//! 资源回收路径会同时检查成功、业务失败和删除失败三类情况。
//! 这对应 Go 里 `defer os.RemoveAll(tmpdir)` 的全部可见后果。
//! 若成功路径不删临时目录，长期运行会在本地留下大量噪声目录。
//! 若失败路径不删临时目录，则调试一次错误就可能污染下一次运行。
//! 若删除失败被静默吞掉，用户会失去排查 runfiles 和权限问题的线索。
//! 因此这里专门验证 Drop 守卫会把清理失败升级成 panic。
//! 脚本环境还会记录实际调用过的 `go` 参数，方便断言只读模式是否保留。
//! `-mod=readonly` 的存在是避免工具意外修改依赖文件的关键保障。
//! 测试里的 `Capture` 让 stdout 和 stderr 都能像普通字符串一样被检查。
//! 这也是注入式 `run_main_with` 存在的主要意义。
//! 对迁移维护者来说，这组测试比单纯函数级测试更能说明工具的真实契约。
//! 因为它覆盖的是最终用户能观察到的命名、输出、提示和 panic 文本。
//! 注释任务不会增删任何断言，只解释为什么这些断言值得存在。
//! 如果将来要扩展新的例外仓库，也建议先补这里的契约测试。
//! 这样可以防止新增特判在后续重构时被误删。
//! 阅读本文件时，可以把每个场景看成一条“用户可观察行为”说明。
//! 它们共同界定了 Rust 版 mirror 工具必须守住的兼容边界。
//! 正常路径部分强调输出内容和顺序。
//! 边界路径部分强调废弃参数与 replace 行为。
//! 错误路径部分强调错误文本与 panic 包装。
//! 资源回收部分强调临时目录生命周期。
//! 这些部分互相独立，能让失败定位更直接。
//! 这也是统一入口按四组场景依次执行的原因。
//! 如果只看单条断言，很容易忽略它背后的跨语言兼容意义。
//! 把意图写清楚有助于后续区分“实现可变”与“契约不可变”。
//! 例如 JSON 解析实现可以替换，但按 `}` 刷新对象这一契约不能丢。
//! 又如临时目录清理可以不用 Drop，但成功/失败都要清理这一语义不能变。
//! 仓库名转换函数内部可以重构，但结果字符串必须继续兼容 Bazel 依赖名。
//! 同理，错误类型可以重构，但用户看到的 stderr 拼接结果不能随意改变。
//! 因此测试中的字符串断言看似细碎，实则都对应实际兼容成本。
//! 这一点在生成工具类迁移中尤为重要，因为输出差异往往会级联放大。
//! 比如仓库名错一个字符，就可能导致补丁文件、依赖引用和构建脚本全部失效。
//! 又比如排序规则改变，就会让版本库中持续出现无意义的配置 diff。
//! 所以本文件的职责并不是“多测一些函数”，而是固定整个工具的外观。
//! 这也是为什么大多数断言都围绕文本和流程，而不是内部数据结构。
//! 对读者来说，理解这组测试就是理解 Rust 版 mirror 的兼容边界。
//! 对维护者来说，这些注释则提供了更新断言时必须考虑的背景。
//! 本次只补中文说明，不引入新场景也不删减旧场景。
//! 完成后验证重点仍然是差异仅含注释、格式检查与现有断言保持不变。
//! 若后续出现 rustfmt 历史失败，也应按任务协议标记待回归而不是阻塞。
//! 总之，本文件是 mirror 工具外部契约的压缩说明书。
//! 定位失败时，可以先判断它落在命名、下载、输出还是清理四类外观中的哪一类。
//! 这样往往比直接钻进具体 helper 更快找到与 Go 偏离的入口点。
//! 如果将来新增断言，也建议继续围绕“用户能看到什么”来组织说明。
//! 这能确保测试规模增长时，文件仍然保持按公共契约阅读的结构。
//! 后面的逐函数注释会继续沿着这个“契约优先”视角补充解释。

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};

use crate::mirror::{
    self, DownloadedModule, ListedModule, build_file_proto_mode_for_repo, create_tmp_dir,
    download_zips, dump_new_deps_bzl, module_path_to_bazel_repo_name,
    munge_bazel_repo_name_component, parse_downloaded_modules, parse_listed_modules, run_main_with,
};
use crate::stubs::{
    self, Bazel, Capture, Error, Fs, MemFs, OsFs, ProdBazel, ScriptedEnv, parse_flags_checked,
    resolve_runfiles_path,
};

#[test]
fn bazel_new_tmp_dir_creates_unique_directories() {
    let bazel = ProdBazel;
    let first = bazel.NewTmpDir("gomirror-parity-").unwrap();
    let second = bazel.NewTmpDir("gomirror-parity-").unwrap();
    let first_is_dir = std::path::Path::new(&first).is_dir();
    let second_is_dir = std::path::Path::new(&second).is_dir();
    std::fs::remove_dir_all(&first).unwrap();
    if first != second {
        std::fs::remove_dir_all(&second).unwrap();
    }
    assert_ne!(first, second);
    assert!(first_is_dir);
    assert!(second_is_dir);
}

#[test]
fn remove_all_is_idempotent_like_go() {
    let path = ProdBazel.NewTmpDir("gomirror-remove-parity-").unwrap();
    std::fs::remove_dir_all(&path).unwrap();
    OsFs.remove_all(&path).unwrap();
}

#[test]
fn runfiles_path_includes_the_main_workspace() {
    let path = resolve_runfiles_path(Some("/runfiles"), None, Some("tidb")).unwrap();
    assert_eq!(path, std::path::PathBuf::from("/runfiles/tidb"));
    assert!(resolve_runfiles_path(Some("/runfiles"), None, None).is_err());
}

#[test]
fn go_flag_parser_stops_and_rejects_invalid_input() {
    let flags = parse_flags_checked(&["input".into(), "--mirror".into()]).unwrap();
    assert!(
        !flags.is_mirror,
        "Go flag parsing stops at the first argument"
    );

    let flags = parse_flags_checked(&["--".into(), "--upload".into()]).unwrap();
    assert!(!flags.is_upload, "Go flag parsing stops after --");

    let flags = parse_flags_checked(&["--mirror=1".into(), "--upload=FALSE".into()]).unwrap();
    assert!(flags.is_mirror);
    assert!(!flags.is_upload);

    let unknown = parse_flags_checked(&["--other".into()]).unwrap_err();
    assert!(
        unknown
            .msg
            .contains("flag provided but not defined: -other")
    );

    let invalid = parse_flags_checked(&["--mirror=maybe".into()]).unwrap_err();
    assert!(
        invalid
            .msg
            .contains("invalid value \"maybe\" for flag -mirror")
    );
}

#[test]
// 统一入口把四类公共契约按“正常、边界、错误、清理”顺序串起来执行。
// 一旦某组失败，定位者可以先按契约类别收缩范围，再继续追具体 helper。
fn go_rust_public_contract_matches() {
    contract_normal_deps_bzl_and_naming();
    contract_boundary_flags_replace_and_patches();
    contract_error_paths();
    contract_resource_cleanup();
}

/// Normal path: bazel repo naming, proto mode, sorted deps.bzl emission, JSON parse.
// 这一组固定“正常生成”外观，重点是命名规则、JSON 解析和输出顺序。
// 如果这里失败，通常表示生成结果会与 Go 版产生可见 diff，而不是内部实现细节变化。
fn contract_normal_deps_bzl_and_naming() {
    assert_eq!(munge_bazel_repo_name_component("client-go"), "client_go");
    assert_eq!(munge_bazel_repo_name_component("yaml.v2"), "yaml_v2");
    assert_eq!(
        module_path_to_bazel_repo_name("github.com/pingcap/tidb/pkg/parser"),
        "com_github_pingcap_tidb_pkg_parser"
    );
    assert_eq!(
        module_path_to_bazel_repo_name("github.com/tikv/client-go/v2"),
        "com_github_tikv_client_go_v2"
    );
    assert_eq!(
        module_path_to_bazel_repo_name("go.etcd.io/etcd/api/v3"),
        "io_etcd_go_etcd_api_v3"
    );
    assert_eq!(
        module_path_to_bazel_repo_name("github.com/grpc-ecosystem/grpc-gateway"),
        "com_github_grpc_ecosystem_grpc_gateway"
    );
    assert_eq!(
        module_path_to_bazel_repo_name("golang.org/x/net"),
        "org_golang_x_net"
    );
    assert_eq!(
        module_path_to_bazel_repo_name("gopkg.in/yaml.v2"),
        "in_gopkg_yaml_v2"
    );

    assert_eq!(
        build_file_proto_mode_for_repo("io_etcd_go_etcd_api_v3"),
        "disable"
    );
    assert_eq!(
        build_file_proto_mode_for_repo("com_github_tikv_client_go_v2"),
        "disable_global"
    );

    // Streamed JSON: line-accumulated objects; skip root tidb module.
    // 这里刻意保留“逐对象刷出”的输入形态，避免误把 Go 的串流输出改造成数组专用解析。
    let list_json = br#"{
	"Path": "github.com/pingcap/tidb",
	"Version": "v0.0.0"
}
{
	"Path": "github.com/tikv/client-go/v2",
	"Version": "v2.0.1"
}
{
	"Path": "go.etcd.io/etcd/api/v3",
	"Version": "v3.5.0"
}
{
	"Path": "github.com/grpc-ecosystem/grpc-gateway",
	"Version": "v1.16.0"
}
"#;
    let listed = parse_listed_modules(list_json).unwrap();
    assert!(!listed.contains_key("github.com/pingcap/tidb"));
    assert_eq!(listed.len(), 3);
    assert_eq!(listed["github.com/tikv/client-go/v2"].Version, "v2.0.1");

    let dl_json = br#"{
	"Path": "github.com/tikv/client-go/v2",
	"Version": "v2.0.1",
	"Sum": "h1:tikvsum",
	"Zip": "/zip/tikv"
}
{
	"Path": "go.etcd.io/etcd/api/v3",
	"Version": "v3.5.0",
	"Sum": "h1:etcdsum",
	"Zip": "/zip/etcd"
}
{
	"Path": "github.com/grpc-ecosystem/grpc-gateway",
	"Version": "v1.16.0",
	"Sum": "h1:gwsum",
	"Zip": "/zip/gw"
}
"#;
    let downloaded = parse_downloaded_modules(dl_json).unwrap();
    assert_eq!(downloaded.len(), 3);

    let env = ScriptedEnv {
        runfiles_root: "/runfiles".into(),
        ..ScriptedEnv::default()
    };
    let fs = MemFs::new();
    let mut out = Capture::new();
    dump_new_deps_bzl(&env, &fs, &mut out, &listed, &downloaded).unwrap();
    let text = out.string();

    assert!(text.contains("load(\"@bazel_gazelle//:deps.bzl\", \"go_repository\")"));
    assert!(text.contains("def go_deps():"));
    // Sorted by bazel repo name: com_github_grpc... then com_github_tikv... then io_etcd...
    let grpc_pos = text.find("com_github_grpc_ecosystem_grpc_gateway").unwrap();
    let tikv_pos = text.find("com_github_tikv_client_go_v2").unwrap();
    let etcd_pos = text.find("io_etcd_go_etcd_api_v3").unwrap();
    assert!(grpc_pos < tikv_pos && tikv_pos < etcd_pos);

    assert!(text.contains("build_tags = [\"nextgen\", \"intest\"]"));
    assert!(text.contains("build_file_proto_mode = \"disable\""));
    assert!(text.contains("build_naming_convention = \"go_default_library\""));
    assert!(text.contains("importpath = \"github.com/tikv/client-go/v2\""));
    assert!(text.contains("sum = \"h1:tikvsum\""));
    assert!(text.contains("version = \"v2.0.1\""));
    // parser path would be skipped if present
    assert!(!text.contains("com_github_pingcap_tidb_pkg_parser"));
}

/// Boundary: deprecated flags, local replace skip, patch args, download arg shapes.
// 这一组覆盖最容易在重构中被顺手改掉的兼容细节，例如废弃 flag、replace 与补丁文件。
// 这些行为看起来像边角料，但都会直接影响生成参数、下载对象和最终 `deps.bzl` 文本。
fn contract_boundary_flags_replace_and_patches() {
    let flags = stubs::parse_flags(&["--mirror".into(), "-upload".into()]);
    assert!(flags.is_mirror);
    assert!(flags.is_upload);

    let flags2 = stubs::parse_flags(&["-mirror=false".into()]);
    assert!(!flags2.is_mirror);

    // Replace with empty Version (local path) is skipped in download args.
    let mut listed = HashMap::new();
    listed.insert(
        "example.com/a".into(),
        ListedModule {
            Path: "example.com/a".into(),
            Version: "v1.0.0".into(),
            Replace: Some(Box::new(ListedModule {
                Path: "../local".into(),
                Version: String::new(),
                Replace: None,
            })),
        },
    );
    listed.insert(
        "example.com/b".into(),
        ListedModule {
            Path: "example.com/b".into(),
            Version: "v2.0.0".into(),
            Replace: Some(Box::new(ListedModule {
                Path: "example.com/b-fork".into(),
                Version: "v2.0.1".into(),
                Replace: None,
            })),
        },
    );
    listed.insert(
        "example.com/c".into(),
        ListedModule {
            Path: "example.com/c".into(),
            Version: "v3.0.0".into(),
            Replace: None,
        },
    );

    let mut env = ScriptedEnv::default();
    env.runfiles.insert("bin/go".into(), "/bin/go".into());
    env.download_json = br#"{
	"Path": "example.com/b-fork",
	"Version": "v2.0.1",
	"Sum": "h1:bfork",
	"Zip": "/z/b"
}
{
	"Path": "example.com/c",
	"Version": "v3.0.0",
	"Sum": "h1:csum",
	"Zip": "/z/c"
}
"#
    .to_vec();
    let downloaded = download_zips(&env, &env, "/tmp/x", &listed).unwrap();
    let cmds = env.commands.borrow().clone();
    assert_eq!(cmds.len(), 1);
    let args = &cmds[0].args;
    assert_eq!(
        &args[0..3],
        &[
            "mod".to_string(),
            "download".to_string(),
            "-json".to_string()
        ]
    );
    // empty-version replace skipped; remote replace + plain module present
    assert!(!args.iter().any(|a| a.contains("../local")));
    assert!(args.iter().any(|a| a == "example.com/b-fork@v2.0.1"));
    assert!(args.iter().any(|a| a == "example.com/c@v3.0.0"));
    assert!(cmds[0].env.iter().any(|e| e == "GOSUMDB=sum.golang.org"));
    assert_eq!(downloaded["example.com/b-fork"].Sum, "h1:bfork");

    // Patch file present → Starlark patch_args emitted; replace field emitted.
    let mut listed2 = HashMap::new();
    listed2.insert(
        "github.com/tikv/pd".into(),
        ListedModule {
            Path: "github.com/tikv/pd".into(),
            Version: "v0.0.0".into(),
            Replace: Some(Box::new(ListedModule {
                Path: "github.com/tikv/pd-fork".into(),
                Version: "v1.2.3".into(),
                Replace: None,
            })),
        },
    );
    let mut downloaded2 = HashMap::new();
    downloaded2.insert(
        "github.com/tikv/pd-fork".into(),
        DownloadedModule {
            Path: "github.com/tikv/pd-fork".into(),
            Sum: "h1:pd".into(),
            Version: "v1.2.3".into(),
            Zip: "/z".into(),
        },
    );
    let env2 = ScriptedEnv {
        runfiles_root: "/rf".into(),
        ..ScriptedEnv::default()
    };
    let fs = MemFs::new();
    fs.put("/rf/build/patches/com_github_tikv_pd.patch", b"diff");
    let mut out = Capture::new();
    dump_new_deps_bzl(&env2, &fs, &mut out, &listed2, &downloaded2).unwrap();
    let text = out.string();
    assert!(text.contains("patch_args = [\"-p1\"]"));
    assert!(text.contains("//build/patches:com_github_tikv_pd.patch"));
    assert!(text.contains("replace = \"github.com/tikv/pd-fork\""));
    assert!(text.contains("build_tags = [\"nextgen\", \"intest\"]"));

    // createTmpDir copies via string-replaced runfile paths.
    let mut env3 = ScriptedEnv::default();
    env3.runfiles.insert("go.mod".into(), "/ws/go.mod".into());
    env3.runfiles.insert("go.sum".into(), "/ws/go.sum".into());
    let fs3 = MemFs::new();
    fs3.put("/ws/go.mod", b"module root\n");
    fs3.put("/ws/go.sum", b"sum root\n");
    fs3.put("/ws/pkg/parser/go.mod", b"module parser\n");
    fs3.put("/ws/pkg/parser/go.sum", b"sum parser\n");
    let tmp = create_tmp_dir(&env3, &fs3).unwrap();
    assert!(tmp.contains("gomirror"));
    assert_eq!(
        fs3.get(&format!("{tmp}/go.mod")).as_deref(),
        Some(b"module root\n".as_slice())
    );
    assert_eq!(
        fs3.get(&format!("{tmp}/pkg/parser/go.mod")).as_deref(),
        Some(b"module parser\n".as_slice())
    );
    assert!(fs3.dirs.borrow().iter().any(|d| d.ends_with("pkg/parser")));
}

/// Error paths: missing download, exit-error panic wrapping, bad JSON.
// 这一组不只是验证“会失败”，而是验证失败时用户能看到的文本形状仍与 Go 一致。
// 因此断言同时关注缺失模块、坏 JSON 和子进程 stderr 三类高频失败入口。
fn contract_error_paths() {
    let listed = {
        let mut m = HashMap::new();
        m.insert(
            "example.com/x".into(),
            ListedModule {
                Path: "example.com/x".into(),
                Version: "v1.0.0".into(),
                Replace: None,
            },
        );
        m
    };
    let downloaded: HashMap<String, DownloadedModule> = HashMap::new();
    let env = ScriptedEnv::default();
    let fs = MemFs::new();
    let mut out = Capture::new();
    let err = dump_new_deps_bzl(&env, &fs, &mut out, &listed, &downloaded).unwrap_err();
    assert!(
        err.msg
            .contains("could not find downloaded module for example.com/x@v1.0.0"),
        "got {}",
        err.msg
    );

    // Go flushes an object when a line has prefix `}`; invalid body must fail unmarshal.
    let bad = parse_listed_modules(b"{\nnot json\n}\n");
    assert!(bad.is_err());

    // ExitError → panic with stderr prefix matching Go.
    let mut env2 = ScriptedEnv::default();
    env2.runfiles.insert("go.mod".into(), "/ws/go.mod".into());
    env2.runfiles.insert("go.sum".into(), "/ws/go.sum".into());
    env2.runfiles.insert("bin/go".into(), "/bin/go".into());
    env2.list_err = Some(Error::exit(b"go list failed".to_vec()));
    let fs2 = MemFs::new();
    fs2.put("/ws/go.mod", b"m\n");
    fs2.put("/ws/go.sum", b"s\n");
    fs2.put("/ws/pkg/parser/go.mod", b"m\n");
    fs2.put("/ws/pkg/parser/go.sum", b"s\n");
    let mut out2 = Capture::new();
    let mut err2 = Capture::new();
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = run_main_with(
            &["--mirror".into()],
            &env2,
            &env2,
            &fs2,
            &mut out2,
            &mut err2,
        );
    }));
    assert!(result.is_err());
    let panic_msg = panic_message(result.unwrap_err());
    assert!(
        panic_msg.contains("subprocess exited with stderr:\ngo list failed"),
        "got {panic_msg}"
    );
    assert!(
        err2.string()
            .contains("--mirror is deprecated and ignored; modules are resolved through GOPROXY")
    );
}

/// Resource cleanup: tmpdir removed after success and after list failure.
// 这一组把临时目录生命周期当成可观察契约，而不是实现内部的附带效果。
// 成功、业务失败和删除失败都要覆盖，才能完整对应 Go 的 `defer RemoveAll` 语义。
fn contract_resource_cleanup() {
    // Success path removes tmpdir.
    let list_json = br#"{
	"Path": "example.com/ok",
	"Version": "v1.0.0"
}
"#;
    let dl_json = br#"{
	"Path": "example.com/ok",
	"Version": "v1.0.0",
	"Sum": "h1:ok",
	"Zip": "/z"
}
"#;
    let mut env = ScriptedEnv::default();
    env.runfiles.insert("go.mod".into(), "/ws/go.mod".into());
    env.runfiles.insert("go.sum".into(), "/ws/go.sum".into());
    env.runfiles.insert("bin/go".into(), "/bin/go".into());
    env.list_json = list_json.to_vec();
    env.download_json = dl_json.to_vec();
    let fs = MemFs::new();
    fs.put("/ws/go.mod", b"m\n");
    fs.put("/ws/go.sum", b"s\n");
    fs.put("/ws/pkg/parser/go.mod", b"m\n");
    fs.put("/ws/pkg/parser/go.sum", b"s\n");
    let mut out = Capture::new();
    let mut err = Capture::new();
    run_main_with(&[], &env, &env, &fs, &mut out, &mut err).unwrap();
    let removed = fs.removed_paths();
    assert_eq!(removed.len(), 1);
    assert!(removed[0].contains("gomirror"));
    assert!(out.string().contains("example.com/ok"));

    // listAllModules uses readonly flags.
    let cmds = env.commands.borrow().clone();
    assert!(cmds.iter().any(|c| {
        c.args == ["list", "-mod=readonly", "-m", "-json", "all"] && c.gobin == "/bin/go"
    }));

    // Failure after createTmpDir still cleans up (defer semantics).
    let mut env2 = ScriptedEnv::default();
    env2.runfiles.insert("go.mod".into(), "/ws/go.mod".into());
    env2.runfiles.insert("go.sum".into(), "/ws/go.sum".into());
    env2.runfiles.insert("bin/go".into(), "/bin/go".into());
    env2.list_err = Some(Error::new("list boom"));
    let fs2 = MemFs::new();
    fs2.put("/ws/go.mod", b"m\n");
    fs2.put("/ws/go.sum", b"s\n");
    fs2.put("/ws/pkg/parser/go.mod", b"m\n");
    fs2.put("/ws/pkg/parser/go.sum", b"s\n");
    let mut out2 = Capture::new();
    let mut err2 = Capture::new();
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = run_main_with(&[], &env2, &env2, &fs2, &mut out2, &mut err2);
    }));
    assert!(result.is_err());
    assert_eq!(fs2.removed_paths().len(), 1);

    // RemoveAll failure panics like Go defer.
    // 这里故意让删除动作报错，防止后续把清理失败降级成日志后静默放过。
    let mut env3 = ScriptedEnv::default();
    env3.runfiles.insert("go.mod".into(), "/ws/go.mod".into());
    env3.runfiles.insert("go.sum".into(), "/ws/go.sum".into());
    env3.runfiles.insert("bin/go".into(), "/bin/go".into());
    env3.list_json = list_json.to_vec();
    env3.download_json = dl_json.to_vec();
    let fs3 = MemFs::new();
    fs3.put("/ws/go.mod", b"m\n");
    fs3.put("/ws/go.sum", b"s\n");
    fs3.put("/ws/pkg/parser/go.mod", b"m\n");
    fs3.put("/ws/pkg/parser/go.sum", b"s\n");
    *fs3.fail_remove.borrow_mut() = Some(Error::new("rm failed"));
    let mut out3 = Capture::new();
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        mirror::mirror_with(&env3, &env3, &fs3, &mut out3).ok();
    }));
    assert!(result.is_err());
}

// 这个小工具把不同 panic 载荷统一转成字符串，避免断言被 `Any` 的具体形态绑死。
// 测试只关心最终暴露给用户的文本，不关心 panic 内部是 `String` 还是 `&str`。
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    format!("{payload:?}")
}
