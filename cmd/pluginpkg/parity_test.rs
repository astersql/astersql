// Copyright 2026 AsterSQL.

//! Parity tests for `cmd/pluginpkg` public contracts vs Go `pluginpkg.go`.
//!
//! 这组测试不是在验证某个单一函数的返回值，而是在锁定整个命令行打包流程
//! 对外暴露出来的契约是否继续和 Go 版一致。
//! 之所以集中写成契约测试，是因为 `pluginpkg` 依赖文件系统、时钟、子进程
//! 和标准输出；如果只测零碎 helper，很容易漏掉 Go 版行为中那些跨边界的细节。
//! 测试通过替身收集命令、文件写入、日志和标准输出，再把这些观测结果和
//! `pluginpkg.go` 的真实控制流逐段对齐。
//! 这里特别关注四类容易在移植时漂移的语义：
//! 1. `go build` 参数顺序、工作目录和额外环境变量。
//! 2. 模板渲染时哪些字段会被省略，哪些字段必须保留。
//! 3. 失败路径是否像 Go 一样走 `os.Exit` 语义，从而跳过延迟清理。
//! 4. 成功路径打印的清单 JSON 是否仍然保留可供人工核对的格式。
//! 由于测试目标是“公开契约”，断言更偏向外部可观察结果，而不是内部实现细节。
//! 这样即使未来内部重构，只要仍满足 Go 对齐约束，测试就不需要跟着脆弱变化。

use std::panic::{self, AssertUnwindSafe};

use serde_json::json;

use crate::pluginpkg::{
    self, CODE_TEMPLATE, build_go_flags, decode_manifest_toml, encode_manifest_json,
    execute_code_template, run_with, template_truthy,
};
use crate::stubs::{self, Capture, Error, FixedClock, Flags, MemFs, ScriptedRunner};
use crate::stubs::{Clock, SystemClock};
use crate::stubs::{ProdRunner, Runner};

const SAMPLE_MANIFEST: &str = r#"
name = "conn_ip_example"
kind = "Audit"
description = "just a test"
version = "1"
license = ""
validate = "Validate"
onInit = "OnInit"
onShutdown = "OnShutdown"
export = [
    {extPoint="OnGeneralEvent", impl="OnGeneralEvent"},
    {extPoint="OnConnectionEvent", impl="OnConnectionEvent"}
]
"#;

/// 入口测试只承担“编排器”角色。
/// Go 版没有对应的总测试函数，因此这里用一个顶层测试把四个子场景串起来，
/// 方便在 `cargo test` 中把本文件视为一份完整的对齐清单。
/// 这样做还能确保任何一个契约退化时，调用栈仍然停留在当前文件，便于定位。
#[test]
fn go_rust_public_contract_matches() {
    contract_normal_packaging();
    contract_boundary_flags_and_template();
    contract_error_paths();
    contract_resource_cleanup();
}

/// 中文补充：这一组验证“主干成功路径”。
/// 它覆盖从读取 manifest 到生成 `.so` 成功提示的整条链路，
/// 对应 Go `main` 中最常见也最关键的执行分支。
/// 这里不直接调用真实文件系统和 `go build`，而是通过替身锁定
/// 打包器承诺给外界的副作用。
/// 如果未来重构改变了内部实现，但这些外部观测仍保持一致，
/// 该测试就应继续通过。
/// Normal path: template render, go build flags, success output, JSON manifest.
fn contract_normal_packaging() {
    let fs = MemFs::new();
    let pkg = "/plugins/conn_ip_example";
    let out = "/out";
    fs.put(&format!("{pkg}/manifest.toml"), SAMPLE_MANIFEST);

    let runner = ScriptedRunner::new();
    let clock = FixedClock {
        value: "2026-07-27 00:00:00 +0000 UTC m=+0.000000000".into(),
    };
    let mut log = Capture::new();
    let mut stdout = Capture::new();

    // 这里显式构造和 Go 命令行一致的空 `pgo_file` / `next_gen=false`，
    // 目的是让后面的断言只聚焦默认路径，不混入边界开关影响。
    let flags = Flags {
        pkg_dir: pkg.into(),
        out_dir: out.into(),
        pgo_file: String::new(),
        next_gen: false,
    };
    run_with(
        flags,
        &fs,
        &runner,
        &clock,
        &mut log,
        &mut stdout,
        "pluginpkg",
    );

    // 断言命令条数为 1，表示成功路径只会发起一次编译。
    // 如果未来出现重复编译、预检查命令或清理命令混入，这里会第一时间暴露。
    // go build args match Go flags construction.
    let cmds = runner.commands();
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].program, "go");
    assert_eq!(cmds[0].dir, pkg);
    assert_eq!(cmds[0].env, vec!["GO111MODULE=on".to_string()]);
    assert_eq!(
        cmds[0].args,
        vec![
            "build".to_string(),
            "-tags=codes".to_string(),
            "-buildmode=plugin".to_string(),
            "-o".to_string(),
            format!("{out}/conn_ip_example-1.so"),
            pkg.to_string(),
        ]
    );

    // Go 成功返回时会触发 `defer os.Remove`，因此 Rust 版也必须在成功路径删除
    // 临时生成的 `.gen.go`。这里同时检查当前文件不存在和删除记录已出现，
    // 防止实现只做“逻辑删除”或只更新某一侧状态。
    // Gen file removed on success (Go defer on normal return).
    assert!(
        fs.get(&format!("{pkg}/conn_ip_example.gen.go")).is_none(),
        "gen.go must be removed after success"
    );
    assert!(
        fs.removed_paths()
            .iter()
            .any(|p| p.ends_with("conn_ip_example.gen.go"))
    );

    // 标准输出既是用户可见协议，也是脚本可能依赖的文本界面。
    // 因此这里不仅检查 success 字样，还保留对 Manifest 头和关键字段的断言，
    // 避免未来把输出悄悄改成别的格式却没有测试失败。
    let out_s = stdout.string();
    assert!(
        out_s.contains(&format!(
            "Package \"{pkg}\" as plugin \"{out}/conn_ip_example-1.so\" success."
        )),
        "success banner: {out_s}"
    );
    assert!(out_s.contains("Manifest:\n"), "manifest header");
    // JSON body contains injected buildTime and original fields.
    assert!(out_s.contains("\"name\""));
    assert!(out_s.contains("conn_ip_example"));
    assert!(out_s.contains("2026-07-27 00:00:00 +0000 UTC m=+0.000000000"));

    // 这里重新执行模板，而不是去读已被删除的临时文件。
    // 这样既能验证模板输出本身，又不会破坏上面对“成功后必须删除 gen.go”的契约。
    // 注入固定时钟是为了把 Go `time.Now().String()` 造成的不稳定性消掉，
    // 让断言只聚焦模板内容是否和 Go 版一致。
    // Template content contract (from last written gen before remove — re-render).
    let mut manifest = decode_manifest_toml(SAMPLE_MANIFEST).unwrap();
    manifest.insert(
        "buildTime".into(),
        serde_json::Value::String(clock.value.clone()),
    );
    let generated = execute_code_template(&manifest).unwrap();
    assert!(generated.contains("plugin.AuditManifest"));
    assert!(generated.contains("Kind:           plugin.Audit"));
    assert!(generated.contains("Name:           \"conn_ip_example\""));
    assert!(generated.contains("Version:        1,"));
    assert!(generated.contains("Validate:   Validate,"));
    assert!(generated.contains("OnInit:     OnInit,"));
    assert!(generated.contains("OnShutdown: OnShutdown,"));
    assert!(generated.contains("OnGeneralEvent: OnGeneralEvent,"));
    assert!(generated.contains("OnConnectionEvent: OnConnectionEvent,"));
    assert!(!generated.contains("OnFlush:"), "empty onFlush omitted");
    assert!(CODE_TEMPLATE.contains("PluginManifest"));
}

/// 中文补充：这一组覆盖“边界输入”和“可选字段”。
/// 它的目的不是重复成功路径，而是锁定最容易在参数解析和模板真值判断上
/// 发生细微偏差的行为。
/// Go 的 flag 包接受 `-name value` 和 `-name=value` 两种写法，
/// Rust 替身解析器如果只支持其中一种，就会在真实命令行中出现兼容性回退。
/// 同时，Go `text/template` 对空字符串、空数组和缺失字段的 truthiness
/// 与很多模板引擎不同，这里必须单独钉死。
/// Boundaries: next-gen tag, pgo-file, optional template fields, flag parse, truthiness.
fn contract_boundary_flags_and_template() {
    // 先验证 flag 解析，因为后续所有路径都建立在它能还原 Go CLI 语法之上。
    // Flag parse: Go flag accepts -name value and -name=value forms.
    let f = stubs::parse_flags(&[
        "--pkg-dir".into(),
        "/p".into(),
        "-out-dir=/o".into(),
        "--pgo-file".into(),
        "prof.pgo".into(),
        "-next-gen".into(),
    ]);
    assert_eq!(f.pkg_dir, "/p");
    assert_eq!(f.out_dir, "/o");
    assert_eq!(f.pgo_file, "prof.pgo");
    assert!(f.next_gen);

    // `build_go_flags` 负责把结构化输入还原成 Go 子进程参数列表。
    // 这里检查的是参数位置和拼接规则，而不是只看最终字符串包含某个片段，
    // 因为 `go build` 某些参数对顺序敏感，粗粒度断言会掩盖回归。
    let fs = MemFs::new();
    let (out, flags) = build_go_flags(
        "/plugins/conn_ip_example",
        "/out",
        "conn_ip_example",
        "1",
        "/abs/prof.pgo",
        true,
        &fs,
    );
    assert_eq!(out, "/out/conn_ip_example-1.so");
    assert_eq!(flags[0], "build");
    assert_eq!(flags[1], "-pgo=/abs/prof.pgo");
    assert_eq!(flags[2], "-tags=codes,nextgen");
    assert_eq!(flags[3], "-buildmode=plugin");

    // 这里刻意省略 `validate` / `onInit` / `onShutdown` / `onFlush`，
    // 用来确认 Rust 模板执行器和 Go 一样不会输出空钩子字段。
    // 如果把这些空字段打印出来，生成的 Go 源码虽然可能还能编译，
    // 但已经偏离原命令生成物。
    // Optional hooks omitted when empty / missing.
    let mut m = serde_json::Map::new();
    m.insert("kind".into(), json!("Audit"));
    m.insert("name".into(), json!("x"));
    m.insert("description".into(), json!("d"));
    m.insert("version".into(), json!(2));
    m.insert("license".into(), json!(""));
    m.insert("buildTime".into(), json!("t"));
    let generated = execute_code_template(&m).unwrap();
    assert!(generated.contains("Version:        2,"));
    assert!(!generated.contains("Validate:"));
    assert!(!generated.contains("OnInit:"));
    assert!(!generated.contains("OnShutdown:"));
    assert!(!generated.contains("OnFlush:"));

    // 真值判断需要和 Go `text/template` 对齐，而不是照搬 Rust/JSON 直觉。
    // 尤其空数组和空字符串在模板里都应视为 false。
    assert!(!template_truthy(None));
    assert!(!template_truthy(Some(&json!(""))));
    assert!(template_truthy(Some(&json!("Validate"))));
    assert!(!template_truthy(Some(&json!([]))));
    assert!(template_truthy(Some(&json!([1]))));

    // 这一段验证相对路径会先被替身文件系统转换为绝对路径，
    // 对齐 Go `filepath.Abs` 在 `main` 里的预处理。
    // 只有这样，后续生成的 `-pgo=` 与 `Dir` 才会和 Go 一样稳定。
    // Relative pkg-dir / out-dir get Abs prefix in MemFs.
    let fs = MemFs::new();
    fs.put(
        "/abs/myplug/manifest.toml",
        SAMPLE_MANIFEST.replace("conn_ip_example", "myplug"),
    );
    // name in sample is conn_ip_example — rewrite fully:
    let manifest = SAMPLE_MANIFEST.replace("conn_ip_example", "myplug");
    fs.put("/abs/myplug/manifest.toml", &manifest);
    let runner = ScriptedRunner::new();
    let clock = FixedClock { value: "t".into() };
    let mut log = Capture::new();
    let mut stdout = Capture::new();
    run_with(
        Flags {
            pkg_dir: "myplug".into(),
            out_dir: "out".into(),
            pgo_file: "p.pgo".into(),
            next_gen: true,
        },
        &fs,
        &runner,
        &clock,
        &mut log,
        &mut stdout,
        "pluginpkg",
    );
    let cmds = runner.commands();
    assert_eq!(cmds[0].dir, "/abs/myplug");
    assert!(cmds[0].args.iter().any(|a| a == "-pgo=/abs/p.pgo"));
    assert!(cmds[0].args.iter().any(|a| a == "-tags=codes,nextgen"));
    // 权限位断言被放到失败路径里完成，
    // 因为成功路径会删除临时文件，无法再读取 mode。
    // Mode 0700 on gen write (before remove).
    // Re-run a write-only path check via MemFs after a failed build below.
}

/// 中文补充：这一组锁定所有“提前退出”的失败语义。
/// 这些场景共同特点是打包流程不会继续执行，但退出前写到日志里的文案
/// 和 Go 行为一样重要，因为运维与调用脚本会依赖这些提示排障。
/// 测试使用 `catch_unwind` 接住替身里的致命退出，目的是把 Go `os.Exit(1)`
/// 映射成可断言的 Rust 测试行为。
/// Error paths: usage, name mismatch, bad toml, go build failure.
fn contract_error_paths() {
    // 缺少必要参数时，Go 会打印 usage 并退出；
    // 这里直接传默认 Flags，验证 Rust 版没有偷偷给字段补默认值。
    // Missing pkg-dir / out-dir → usage exit.
    let fs = MemFs::new();
    let runner = ScriptedRunner::new();
    let clock = FixedClock { value: "t".into() };
    let mut log = Capture::new();
    let mut stdout = Capture::new();
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        run_with(
            Flags::default(),
            &fs,
            &runner,
            &clock,
            &mut log,
            &mut stdout,
            "/bin/pluginpkg",
        );
    }));
    assert!(r.is_err());
    let log_s = log.string();
    assert!(
        log_s.contains("Usage: pluginpkg --pkg-dir"),
        "usage message: {log_s}"
    );

    // manifest 内 `name` 必须和包目录名一致，这是 Go 主程序的硬约束。
    // 这里故意制造目录名与 manifest 名称不一致，确保 Rust 版仍在同一位置失败，
    // 而不是等到后续编译期才报更模糊的错误。
    // Name must equal directory base.
    let fs = MemFs::new();
    fs.put(
        "/plugins/wrong_dir/manifest.toml",
        SAMPLE_MANIFEST, // name=conn_ip_example ≠ wrong_dir
    );
    let mut log = Capture::new();
    let mut stdout = Capture::new();
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        run_with(
            Flags {
                pkg_dir: "/plugins/wrong_dir".into(),
                out_dir: "/out".into(),
                ..Flags::default()
            },
            &fs,
            &runner,
            &clock,
            &mut log,
            &mut stdout,
            "pluginpkg",
        );
    }));
    assert!(r.is_err());
    assert!(
        log.string()
            .contains("plugin package must be same with plugin name in manifest file")
    );

    // TOML 解析失败必须被归类到 manifest 读取/解析错误，
    // 不能静默吞掉，也不能伪装成模板或编译阶段错误。
    // Bad TOML.
    let fs = MemFs::new();
    fs.put("/plugins/conn_ip_example/manifest.toml", "[[[not toml");
    let mut log = Capture::new();
    let mut stdout = Capture::new();
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        run_with(
            Flags {
                pkg_dir: "/plugins/conn_ip_example".into(),
                out_dir: "/out".into(),
                ..Flags::default()
            },
            &fs,
            &runner,
            &clock,
            &mut log,
            &mut stdout,
            "pluginpkg",
        );
    }));
    assert!(r.is_err());
    assert!(log.string().contains("manifest failure"));

    // 这里验证的是 `filepath.Abs` 对应的前置失败。
    // 一旦包路径无法规整成绝对路径，Go 会回退到 usage 分支；
    // Rust 版也必须在进入 manifest 读取前停止，避免额外副作用。
    // Abs failure → usage.
    let fs = MemFs::new();
    fs.fail_abs
        .borrow_mut()
        .insert("bad".into(), Error::new("abs failed"));
    let mut log = Capture::new();
    let mut stdout = Capture::new();
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        run_with(
            Flags {
                pkg_dir: "bad".into(),
                out_dir: "/out".into(),
                ..Flags::default()
            },
            &fs,
            &runner,
            &clock,
            &mut log,
            &mut stdout,
            "pluginpkg",
        );
    }));
    assert!(r.is_err());
    assert!(
        log.string()
            .contains("unable to resolve absolute representation of package path")
    );
}

/// 中文补充：这一组专门锁定“资源清理”这一类最容易被误实现的细节。
/// Go 代码在打开 `.gen.go` 后立刻注册 `defer os.Remove`，但如果后面走到
/// `os.Exit(1)`，这个 defer 并不会执行。
/// Rust 如果简单用 RAII 或统一 finally 语义实现，就会和 Go 版产生实质差异。
/// 因此这里把“成功时删除、构建失败时保留、删除失败只记日志”三种分支分别钉死。
/// Resource cleanup: gen removed on success; retained on go-build failure (Go os.Exit).
fn contract_resource_cleanup() {
    let fs = MemFs::new();
    let pkg = "/plugins/conn_ip_example";
    fs.put(&format!("{pkg}/manifest.toml"), SAMPLE_MANIFEST);
    let runner = ScriptedRunner::new();
    // 让 `go build` 显式失败，模拟 Go `exec.CommandContext(...).Run()` 返回错误。
    *runner.fail.borrow_mut() = Some(Error::exit("exit status 1"));
    let clock = FixedClock { value: "t".into() };
    let mut log = Capture::new();
    let mut stdout = Capture::new();
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        run_with(
            Flags {
                pkg_dir: pkg.into(),
                out_dir: "/out".into(),
                ..Flags::default()
            },
            &fs,
            &runner,
            &clock,
            &mut log,
            &mut stdout,
            "pluginpkg",
        );
    }));
    assert!(r.is_err());
    assert!(log.string().contains("compile plugin source code failure"));
    // 这里是整个文件最关键的差异保护之一：
    // Go 在 `os.Exit` 后不会执行 defer，所以临时文件必须保留下来，
    // 以便证明 Rust 版没有无意中“修复”原有语义。
    // Go os.Exit skips defer → gen file remains.
    let gen_path = format!("{pkg}/conn_ip_example.gen.go");
    let generated = fs
        .get_string(&gen_path)
        .expect("gen.go must remain after build failure");
    assert!(generated.contains("PluginManifest"));
    // 权限位 0700 直接对应 Go `os.OpenFile(..., 0700)`。
    // 即便这里只是临时文件，也需要维持和上游实现一致的可见行为。
    assert_eq!(fs.mode_of(&gen_path), Some(0o700));
    assert!(
        fs.removed_paths().is_empty(),
        "must not remove gen on Exit path"
    );

    // 第二段切换成“成功但删除失败”的场景。
    // 这不是致命错误，Go 只会记录日志并继续向用户报告打包成功；
    // Rust 版也必须保持这种宽松但可追踪的行为。
    // Remove failure on success path only logs (does not fatal).
    let fs = MemFs::new();
    fs.put(&format!("{pkg}/manifest.toml"), SAMPLE_MANIFEST);
    *fs.fail_remove.borrow_mut() = Some(Error::new("permission denied"));
    let runner = ScriptedRunner::new();
    let mut log = Capture::new();
    let mut stdout = Capture::new();
    run_with(
        Flags {
            pkg_dir: pkg.into(),
            out_dir: "/out".into(),
            ..Flags::default()
        },
        &fs,
        &runner,
        &clock,
        &mut log,
        &mut stdout,
        "pluginpkg",
    );
    assert!(
        log.string().contains("remove tmp file")
            && log.string().contains("please clean up manually"),
        "remove failure log: {}",
        log.string()
    );
    assert!(stdout.string().contains("success."));

    // 最后一段补足 JSON 编码器契约。
    // `encode_manifest_json` 虽然可能失败，但成功时应输出结尾换行，
    // 因为 Go `Encoder.Encode` 默认会附带换行，很多文本比较都依赖这一点。
    // encode_manifest_json is fallible but does not abort packaging.
    let mut buf = Capture::new();
    let mut m = serde_json::Map::new();
    m.insert("name".into(), json!("x"));
    encode_manifest_json(&mut buf, &m).unwrap();
    let s = buf.string();
    assert!(s.contains("\"name\""));
    assert!(s.ends_with('\n'));
}

#[test]
fn numeric_manifest_version_is_rejected_like_go_string_assertion() {
    let fs = MemFs::new();
    let pkg = "/plugins/conn_ip_example";
    fs.put(
        &format!("{pkg}/manifest.toml"),
        SAMPLE_MANIFEST.replace("version = \"1\"", "version = 1"),
    );
    let runner = ScriptedRunner::new();
    let clock = FixedClock { value: "t".into() };
    let mut log = Capture::new();
    let mut stdout = Capture::new();

    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        run_with(
            Flags {
                pkg_dir: pkg.into(),
                out_dir: "/out".into(),
                ..Flags::default()
            },
            &fs,
            &runner,
            &clock,
            &mut log,
            &mut stdout,
            "pluginpkg",
        );
    }));

    assert!(
        result.is_err(),
        "Go's manifest[\"version\"].(string) rejects integers"
    );
    assert!(
        runner.commands().is_empty(),
        "invalid version must not invoke go build"
    );
}

#[test]
fn flag_parsing_stops_at_first_positional_argument() {
    let flags = stubs::parse_flags(&[
        "plugin-source".into(),
        "--pkg-dir".into(),
        "/ignored".into(),
        "--next-gen".into(),
    ]);

    assert_eq!(flags, Flags::default());
}

#[test]
fn invalid_flags_fail_instead_of_being_silently_ignored() {
    for args in [
        vec!["--unknown".to_string()],
        vec!["--pkg-dir".to_string()],
        vec!["--next-gen=not-a-bool".to_string()],
    ] {
        let result = panic::catch_unwind(|| stubs::parse_flags(&args));
        assert!(result.is_err(), "Go flag.Parse rejects {args:?}");
    }

    assert_eq!(
        stubs::try_parse_flags(&["--unknown".into()]).unwrap_err(),
        "flag provided but not defined: -unknown"
    );
}

#[test]
fn boolean_flag_accepts_all_go_strconv_spellings() {
    for value in ["1", "t", "T", "TRUE", "true", "True"] {
        let flags = stubs::try_parse_flags(&[format!("--next-gen={value}")]).unwrap();
        assert!(flags.next_gen, "{value}");
    }
    for value in ["0", "f", "F", "FALSE", "false", "False"] {
        let flags = stubs::try_parse_flags(&[format!("--next-gen={value}")]).unwrap();
        assert!(!flags.next_gen, "{value}");
    }
}

#[test]
fn help_flag_requests_usage_without_an_unknown_flag_error() {
    for value in ["-h", "--help"] {
        assert_eq!(stubs::try_parse_flags(&[value.into()]), Err(String::new()));
    }
}

#[cfg(unix)]
#[test]
fn production_runner_formats_nonzero_exit_like_go_exec() {
    let err = ProdRunner
        .run("sh", &["-c".into(), "exit 7".into()], ".", &[])
        .unwrap_err();
    assert_eq!(err.Error(), "exit status 7");
    assert!(err.is_exit);
}

#[test]
fn system_clock_reports_the_current_date_instead_of_unix_epoch() {
    let value = SystemClock.now_string();

    assert!(
        !value.starts_with("1970-01-01"),
        "time.Now().String() must contain the current wall-clock date: {value}"
    );
}

#[test]
fn missing_template_fields_use_go_no_value_text() {
    let mut manifest = decode_manifest_toml(SAMPLE_MANIFEST).unwrap();
    manifest.insert("buildTime".into(), json!("t"));
    manifest.remove("license");

    let generated = execute_code_template(&manifest).unwrap();

    assert!(generated.contains("License:        \"<no value>\""));
}

#[test]
fn manifest_json_uses_go_default_html_escaping() {
    let mut manifest = serde_json::Map::new();
    manifest.insert("description".into(), json!("<tag>&"));
    let mut output = Capture::new();

    encode_manifest_json(&mut output, &manifest).unwrap();

    assert!(output.string().contains(r#"\u003ctag\u003e\u0026"#));
}
