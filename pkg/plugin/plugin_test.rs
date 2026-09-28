// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 插件加载、初始化、查询与 FlushWatcher 循环的单元测试。
//
// 对照 Go `plugin_test.go`：覆盖静态注册优先级、成功/跳过/失败加载路径，
// 以及 Plugins 克隆与 watch 通道退出语义。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serial_test::serial;

use crate::{
    AuditManifest, Config, Context, FlushWatcher, KeyValueClient, Kind, Manifest, Plugin,
    PluginError, Plugins, State, clear_static_plugins, export_manifest, foreach_plugin, get,
    get_all, get_by_name, init, is_enabled, load, load_one_from_dir, register_static_plugin,
    set_test_hook, shutdown,
};

/// 空操作 etcd 桩：get/put/watch 均成功且无数据。
struct DummyEtcd;

impl KeyValueClient for DummyEtcd {
    fn get(&self, _path: &str) -> Result<Option<String>, PluginError> {
        Ok(None)
    }

    fn put(&self, _path: &str, _value: &str) -> Result<(), PluginError> {
        Ok(())
    }

    fn watch(&self, _path: &str) -> Result<Vec<Result<(), PluginError>>, PluginError> {
        Ok(Vec::new())
    }
}

struct OneEventEtcd {
    watched: AtomicBool,
}

impl KeyValueClient for OneEventEtcd {
    fn get(&self, _path: &str) -> Result<Option<String>, PluginError> {
        Ok(None)
    }

    fn put(&self, _path: &str, _value: &str) -> Result<(), PluginError> {
        Ok(())
    }

    fn watch(&self, _path: &str) -> Result<Vec<Result<(), PluginError>>, PluginError> {
        if self.watched.swap(true, Ordering::SeqCst) {
            Ok(Vec::new())
        } else {
            Ok(vec![Ok(())])
        }
    }
}

/// 清空静态注册、测试钩子并 shutdown 全局集合，保证用例隔离。
fn reset_plugin_globals() {
    clear_static_plugins();
    set_test_hook(None);
    shutdown(&Context::default());
}

/// 构造消息为 `"EOF"` 的后端错误，模拟 Go 测试中的 EOF。
fn eof_error() -> PluginError {
    PluginError::backend("EOF")
}

/// 构造认证/审计用 AuditManifest；`fail` 为 true 时钩子一律返回 EOF。
fn audit_manifest(kind: Kind, name: &str, version: u16, fail: bool) -> AuditManifest {
    let mut manifest = Manifest::new(kind, name, version);
    if fail {
        manifest.validate = Some(Arc::new(|_, _| Err(eof_error())));
        manifest.on_init = Some(Arc::new(|_, _| Err(eof_error())));
        manifest.on_shutdown = Some(Arc::new(|_, _| Err(eof_error())));
    } else {
        manifest.validate = Some(Arc::new(|_, _| Ok(())));
        manifest.on_init = Some(Arc::new(|_, _| Ok(())));
        manifest.on_shutdown = Some(Arc::new(|_, _| Ok(())));
    }
    AuditManifest {
        manifest,
        on_general_event: Some(Arc::new(|_, _, _, _| {})),
        on_connection_event: None,
        on_global_variable_event: None,
        on_parse_event: None,
    }
}

/// Corresponds to Go `TestLoadStaticRegisteredPlugin`.
/// 静态注册优先于真实 Open 与测试钩子；静态插件不校验 ID 版本。
#[test]
#[serial]
fn test_load_static_registered_plugin() {
    reset_plugin_globals();

    // 未注册时 load_one 应报与 Go 一致的 plugin.Open 失败信息。
    let err = match load_one_from_dir("/fake/path", "tpluginstatic-1") {
        Ok(_) => panic!("expected missing plugin error"),
        Err(err) => err,
    };
    assert_eq!(
        r#"plugin.Open("/fake/path/tpluginstatic-1.so"): realpath failed"#,
        err.to_string()
    );

    let m = audit_manifest(Kind::Authentication, "tpluginstatic", 1, false);
    let exported = export_manifest(&m);
    let expected_name = exported.name.clone();
    register_static_plugin("tpluginstatic", Arc::new(move || exported.clone()))
        .expect("register static");

    let plugin = load_one_from_dir("/fake/path", "tpluginstatic-1").expect("load static");
    assert_eq!(expected_name, plugin.name());
    assert_eq!(Kind::Authentication, plugin.kind());

    // static plugin do not check version
    // 静态插件不检查 ID 中的版本号。
    let plugin = load_one_from_dir("/fake/path", "tpluginstatic-2").expect("load static v2 id");
    assert_eq!(expected_name, plugin.name());

    // static plugins has a higher priority than the test hook
    // 静态注册优先级高于测试钩子。
    set_test_hook(Some(Arc::new(|_, _| {
        Ok(export_manifest(&audit_manifest(
            Kind::Authentication,
            "tpluginstatic",
            1,
            false,
        )))
    })));
    let plugin = load_one_from_dir("/fake/path", "tpluginstatic-1").expect("static wins");
    assert_eq!(expected_name, plugin.name());

    reset_plugin_globals();
}

/// Corresponds to Go `TestLoadPluginSuccess`.
/// 成功加载并 init 后可查询、遍历；回调返回 EOF 时 foreach 传播错误。
#[test]
#[serial]
fn test_load_plugin_success() {
    reset_plugin_globals();
    let ctx = Context::default();
    let plugin_name = "tplugin";
    let plugin_version: u16 = 1;
    let plugin_sign = format!("{plugin_name}-{plugin_version}");

    let cfg = Config {
        plugins: vec![plugin_sign],
        plugin_dir: String::new(),
        environment_versions: [("go".into(), 1112)].into(),
        ..Config::default()
    };

    set_test_hook(Some(Arc::new(move |_, _| {
        Ok(export_manifest(&audit_manifest(
            Kind::Authentication,
            plugin_name,
            plugin_version,
            false,
        )))
    })));

    load(&ctx, &cfg).expect("load");
    init(&ctx, &cfg).expect("init");

    let ps = get_all().expect("loaded");
    assert_eq!(1, ps.len());
    assert!(is_enabled(Kind::Authentication));

    assert!(get(Kind::Authentication, "tplugin").is_some());
    assert!(get(Kind::Authentication, "tplugin2").is_none());
    assert!(get_by_name("tplugin").is_some());

    foreach_plugin(Kind::Authentication, |_| Ok(())).expect("foreach ok");
    let err = foreach_plugin(Kind::Authentication, |_| Err(eof_error())).expect_err("eof");
    assert_eq!("EOF", err.to_string());

    shutdown(&ctx);
    reset_plugin_globals();
}

/// Corresponds to Go `TestLoadPluginSkipError`.
/// skip_when_fail 时重复 ID / 校验失败 / 不存在插件被跳过，foreach 看不到 Ready 实例。
#[test]
#[serial]
fn test_load_plugin_skip_error() {
    reset_plugin_globals();
    let ctx = Context::default();
    let plugin_name = "tplugin";
    let plugin_version: u16 = 1;
    let plugin_sign = format!("{plugin_name}-{plugin_version}");

    let cfg = Config {
        plugins: vec![plugin_sign.clone(), plugin_sign, "notExists-2".into()],
        plugin_dir: String::new(),
        environment_versions: [("go".into(), 1112)].into(),
        skip_when_fail: true,
        ..Config::default()
    };

    set_test_hook(Some(Arc::new(move |_, _| {
        Ok(export_manifest(&audit_manifest(
            Kind::Audit,
            plugin_name,
            plugin_version,
            true,
        )))
    })));

    load(&ctx, &cfg).expect("load with skip");
    init(&ctx, &cfg).expect("init with skip");
    assert!(!is_enabled(Kind::Audit));

    let ps = get_all().expect("loaded");
    assert_eq!(1, ps.len());

    assert!(get(Kind::Audit, "tplugin").is_some());
    assert!(get(Kind::Audit, "tplugin2").is_none());
    assert!(get_by_name("tplugin").is_some());
    assert!(get_by_name("not exists").is_none());

    let mut ready_count = 0;
    foreach_plugin(Kind::Audit, |_| {
        ready_count += 1;
        Ok(())
    })
    .expect("foreach");
    assert_eq!(0, ready_count);

    shutdown(&ctx);
    reset_plugin_globals();
}

/// Corresponds to Go `TestLoadFail`.
/// 不跳过失败时，重复 ID / 校验失败导致 load 整体返回错误。
#[test]
#[serial]
fn test_load_fail() {
    reset_plugin_globals();
    let ctx = Context::default();
    let plugin_name = "tplugin";
    let plugin_version: u16 = 1;
    let plugin_sign = format!("{plugin_name}-{plugin_version}");

    let cfg = Config {
        plugins: vec![plugin_sign.clone(), plugin_sign, "notExists-2".into()],
        plugin_dir: String::new(),
        environment_versions: [("go".into(), 1112)].into(),
        skip_when_fail: false,
        ..Config::default()
    };

    set_test_hook(Some(Arc::new(move |_, _| {
        Ok(export_manifest(&audit_manifest(
            Kind::Audit,
            plugin_name,
            plugin_version,
            true,
        )))
    })));

    assert!(load(&ctx, &cfg).is_err());
    reset_plugin_globals();
}

/// Corresponds to Go `TestPluginsClone`.
/// clone_plugins 后原集合的原地修改不影响副本。
#[test]
fn test_plugins_clone() {
    let mut ps = Plugins {
        by_kind: [(Kind::Audit, vec![Plugin::default()])].into(),
        versions: [("whitelist".into(), 1)].into(),
        dying_plugins: vec![Plugin::default()],
    };
    let cps = ps.clone_plugins();
    ps.dying_plugins.push(Plugin::default());
    ps.versions.insert("w".into(), 2);
    let as_plugins = ps.by_kind.get(&Kind::Audit).cloned().unwrap_or_default();
    let mut extended = as_plugins;
    extended.push(Plugin::default());
    ps.by_kind.insert(Kind::Audit, extended);

    assert_eq!(1, cps.by_kind.len());
    assert_eq!(
        1,
        cps.by_kind.get(&Kind::Audit).map(|v| v.len()).unwrap_or(0)
    );
    assert_eq!(1, cps.versions.len());
    assert_eq!(Some(&1), cps.versions.get("whitelist"));
    assert_eq!(1, cps.dying_plugins.len());
}

/// Corresponds to Go `TestPluginWatcherLoop`.
/// 取消 Context 时 watch 返回 true；仅关闭通道时返回 false。
#[test]
fn test_plugin_watcher_loop() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let ctx = Context::default();
    let watcher = FlushWatcher::with_context(
        ctx.clone(),
        "test",
        Arc::new(DummyEtcd),
        Manifest::new(Kind::Audit, "test", 1),
        Arc::new(std::sync::atomic::AtomicU32::new(0)),
    );
    let (tx, rx) = mpsc::channel::<()>();
    let cancelled_flag = Arc::clone(&cancelled);
    let ctx_for_thread = ctx.clone();
    // 后台线程先 cancel，保持 tx 存活一段时间，确保退出因取消而非断连。
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        cancelled_flag.store(true, Ordering::SeqCst);
        ctx_for_thread.cancel();
        // Keep `tx` alive so the channel stays open; exit must come from cancel.
        thread::sleep(Duration::from_millis(50));
        drop(tx);
    });
    let exit = watcher.watch_loop_with_chan(&rx);
    assert!(exit);
    assert!(cancelled.load(Ordering::SeqCst));

    let watcher = FlushWatcher::with_context(
        Context::default(),
        "test",
        Arc::new(DummyEtcd),
        Manifest::new(Kind::Audit, "test", 1),
        Arc::new(std::sync::atomic::AtomicU32::new(0)),
    );
    let closed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<()>();
    let closed_flag = Arc::clone(&closed);
    // 仅 drop 发送端，循环应因 Disconnected 返回 false。
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        closed_flag.store(true, Ordering::SeqCst);
        drop(tx);
    });
    let exit = watcher.watch_loop_with_chan(&rx);
    assert!(!exit);
    // Go asserts the first `cancelled` flag (still true), not `closed`.
    // Go 断言的是第一轮留下的 cancelled（仍为 true），而非 closed。
    assert!(cancelled.load(Ordering::SeqCst));
    assert!(closed.load(Ordering::SeqCst));
}

/// Go `Init` starts `flushWatcher.watchLoop` after the initial refresh.
#[test]
#[serial]
fn test_init_starts_plugin_watcher() {
    reset_plugin_globals();
    let ctx = Context::default();
    let flush_count = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&flush_count);
    let mut manifest = Manifest::new(Kind::Audit, "watched", 1);
    manifest.on_flush = Some(Arc::new(move |_, _| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    register_static_plugin("watched", Arc::new(move || manifest.clone()))
        .expect("register watched plugin");
    let cfg = Config {
        plugins: vec!["watched-1".into()],
        etcd: Some(Arc::new(OneEventEtcd {
            watched: AtomicBool::new(false),
        })),
        ..Config::default()
    };

    load(&ctx, &cfg).expect("load watched plugin");
    init(&ctx, &cfg).expect("init watched plugin");
    for _ in 0..100 {
        if flush_count.load(Ordering::SeqCst) >= 2 {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(2, flush_count.load(Ordering::SeqCst));

    shutdown(&ctx);
    reset_plugin_globals();
}

// Silence unused State import warning if any path doesn't use it.
// 压制可能未使用的 State 导入警告。
#[allow(dead_code)]
fn _touch_state() -> State {
    State::Ready
}
