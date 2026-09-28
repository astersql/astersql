// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
// Licensed under the Apache License, Version 2.0 (the "License");

// 本文件对应 Go `pd_serial_test.go`：串行验证 PdController 暂停/恢复与版本门控。
// 网络边界全部 mock，不拨号真实 PD/TiKV，便于在 CI 无集群环境复现。
// 与 parity_test 互补：这里更贴近 Go 原测试用例命名与错误文案断言。
// 失败注入与 label TTL 行为必须与 Go httptest/mock 保持一致，避免假绿。
//! Go-equivalent serial tests from `br/pkg/pdutil/pd_serial_test.go`.
//!
//! Mapping:
//! - `TestScheduler` → `test_scheduler`
//! - `TestPDVersion` → `test_pd_version`
//! - `TestPDResetTSCompatibility` → `test_pd_reset_ts_compatibility`
//! - `TestPauseSchedulersByKeyRange` → `test_pause_schedulers_by_key_range`
//!
// MockPDHTTPClient 扮演 Go mockPDHTTPClient/httptest，覆盖 delay、config、ResetTS、label TTL。
//! Network/PD HTTP boundary is mocked via `PdHttpClient` (same role as Go
//! `mockPDHTTPClient` / `httptest` for label-rule TTL). No real PD/TiKV.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use astersql_br_pkg_errors::Is;
use astersql_errors::SharedError;
use serde_json::Value;

use crate::{
    Context, LabelRule, LabelRulePatch, PdController, PdHttpClient, RegionLabel, StoreInfo,
    parseVersion, pause_scheduler_by_key_range_with_ttl,
};

// 可注入错误与 label 过期表，模拟暂停失败、配置失败与按键范围暂停清理。
/// Mirrors Go `mockPDHTTPClient`: returns a preset error from pause/config/ResetTS.
struct MockPDHTTPClient {
    err: Mutex<Option<String>>,
    // 记录 rule_id → 过期时刻，用于断言 TTL 刷新与取消后清理。
    /// Tracks label-rule TTL expirations like Go `TestPauseSchedulersByKeyRange` httptest.
    label_expires: Mutex<HashMap<String, Instant>>,
    // PATCH 删除后置位，后续 Set/Patch 变成空操作，对齐 httptest 幂等行为。
    deleted: Mutex<bool>,
}

impl MockPDHTTPClient {
    // 构造时可预置错误；None 表示 PD HTTP 调用成功。
    fn with_err(msg: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            err: Mutex::new(msg.map(str::to_string)),
            label_expires: Mutex::new(HashMap::new()),
            deleted: Mutex::new(false),
        })
    }

    // 运行中切换错误注入，覆盖先失败后成功的兼容路径。
    fn set_err(&self, msg: Option<&str>) {
        *self.err.lock().unwrap() = msg.map(str::to_string);
    }

    // 读取当前注入错误；有则转为 SharedError，无则 Ok。
    fn take_err(&self) -> Result<(), SharedError> {
        match self.err.lock().unwrap().as_ref() {
            Some(e) => Err(astersql_errors::New(e.clone())),
            None => Ok(()),
        }
    }
}

// 未测路径返回空/零值；关键路径走 take_err 或 label TTL 逻辑。
impl PdHttpClient for MockPDHTTPClient {
    fn GetClusterVersion(&self, _ctx: &Context) -> Result<String, SharedError> {
        Ok(String::new())
    }
    fn GetPDVersion(&self, _ctx: &Context) -> Result<String, SharedError> {
        Ok(String::new())
    }
    fn GetRegionCountByKeyRange(
        &self,
        _ctx: &Context,
        _start: &[u8],
        _end: &[u8],
    ) -> Result<i32, SharedError> {
        Ok(0)
    }
    fn GetStore(&self, _ctx: &Context, store_id: u64) -> Result<StoreInfo, SharedError> {
        Ok(StoreInfo {
            id: store_id,
            address: String::new(),
        })
    }
    // 串行用例不依赖调度器列表内容，返回空即可。
    fn GetSchedulers(&self, _ctx: &Context) -> Result<Vec<String>, SharedError> {
        Ok(Vec::new())
    }
    // 配置读取在本文件失败路径中未使用，返回空 map。
    fn GetScheduleConfig(&self, _ctx: &Context) -> Result<HashMap<String, Value>, SharedError> {
        Ok(HashMap::new())
    }
    // 配置写入失败路径：直接返回注入错误。
    fn SetConfig(
        &self,
        _ctx: &Context,
        _cfg: &HashMap<String, Value>,
        _ttl_seconds: Option<f64>,
    ) -> Result<(), SharedError> {
        self.take_err()
    }
    // 调度器 delay 失败路径：与 SetConfig 共用 take_err。
    fn SetSchedulerDelay(
        &self,
        _ctx: &Context,
        _name: &str,
        _delay: i64,
    ) -> Result<(), SharedError> {
        self.take_err()
    }
    fn GetRegionLabelRulesByIDs(
        &self,
        _ctx: &Context,
        _ids: &[String],
    ) -> Result<Vec<LabelRule>, SharedError> {
        Ok(Vec::new())
    }
    fn PatchRegionLabelRules(
        &self,
        _ctx: &Context,
        patch: &LabelRulePatch,
    ) -> Result<(), SharedError> {
        // 仅允许删除一条规则；重复 PATCH 在 deleted 后直接成功。
        // Mirrors Go httptest PATCH handler in TestPauseSchedulersByKeyRange.
        let mut deleted = self.deleted.lock().unwrap();
        if *deleted {
            return Ok(());
        }
        // 恢复路径只发 DeleteRules，不应夹带 SetRules。
        assert!(patch.SetRules.is_empty());
        assert_eq!(patch.DeleteRules.len(), 1);
        self.label_expires
            .lock()
            .unwrap()
            .remove(&patch.DeleteRules[0]);
        *deleted = true;
        Ok(())
    }
    fn SetRegionLabelRule(&self, _ctx: &Context, rule: &LabelRule) -> Result<(), SharedError> {
        // 校验 schedule=deny 标签，并按 TTL 维护过期表（0s 表示删除）。
        // Mirrors Go httptest POST handler in TestPauseSchedulersByKeyRange.
        let deleted = self.deleted.lock().unwrap();
        if *deleted {
            return Ok(());
        }
        drop(deleted);

        assert_eq!(rule.Labels.len(), 1);
        let region_label: &RegionLabel = &rule.Labels[0];
        // BR 暂停调度使用固定 label key/value 契约。
        assert_eq!(region_label.Key, "schedule");
        assert_eq!(region_label.Value, "deny");
        // 解析 Go Duration 字符串；刷新时要求旧过期时刻仍未过期。
        let req_ttl = parse_go_duration(&region_label.TTL);
        assert_eq!(
            req_ttl,
            Duration::from_secs(1),
            "label rule TTL must match Go test constant"
        );
        let mut expires = self.label_expires.lock().unwrap();
        // TTL=0 表示立即失效并从过期表移除。
        if req_ttl.is_zero() {
            expires.remove(&rule.ID);
        } else {
            if let Some(expire) = expires.get(&rule.ID) {
                assert!(*expire > Instant::now(), "should not expire before now");
            }
            expires.insert(rule.ID.clone(), Instant::now() + req_ttl);
        }
        Ok(())
    }
    fn GetMinResolvedTSByStoresIDs(
        &self,
        _ctx: &Context,
        _store_ids: Option<&[u64]>,
    ) -> Result<u64, SharedError> {
        Ok(0)
    }
    fn ResetBaseAllocID(&self, _ctx: &Context, _id: u64) -> Result<(), SharedError> {
        Ok(())
    }
    // ResetTS 兼容性测试依赖此处错误注入。
    fn ResetTS(&self, _ctx: &Context, _ts: u64, _force: bool) -> Result<(), SharedError> {
        self.take_err()
    }
    fn SetSnapshotRecoveringMark(&self, _ctx: &Context) -> Result<(), SharedError> {
        Ok(())
    }
    fn DeleteSnapshotRecoveringMark(&self, _ctx: &Context) -> Result<(), SharedError> {
        Ok(())
    }
    // mock Close 无资源可释放。
    fn Close(&self) {}
}

// 测试只覆盖秒级字面量；其他格式直接 panic 暴露契约漂移。
/// Parse Go `time.Duration.String()` forms used by tests (`1s`, `0s`).
fn parse_go_duration(s: &str) -> Duration {
    // 空串与 0s 都视为零时长。
    if s == "0s" || s.is_empty() {
        return Duration::ZERO;
    }
    if let Some(secs) = s.strip_suffix('s') {
        if let Ok(n) = secs.parse::<u64>() {
            return Duration::from_secs(n);
        }
    }
    panic!("unsupported duration string: {s}");
}

// 覆盖：暂停调度器失败、暂停配置失败（ErrPDUpdateFailed）、成功暂停再 Resume。
/// Go `TestScheduler`.
#[test]
fn test_scheduler() {
    let ctx = Context::Background();
    // 选用影响性能的默认调度器名，与 Schedulers() 集合一致。
    let scheduler = "balance-leader-scheduler";
    let mock = MockPDHTTPClient::with_err(Some("failed"));
    let pd = PdController::NewPdControllerWithPDClient(
        None,
        Arc::clone(&mock) as Arc<dyn PdHttpClient>,
        parseVersion("v6.5.0"),
    );
    // Rust 侧 Close 对 nil PD client 安全；pause channel 由构造函数持有。
    // Go: pdController.Client is nil — Close cannot be called; channel closed via drop/Close.
    // Rust constructor owns the pause channel; Close is safe (nil PD client).

    // 首次 pause：注入 failed，期望原样上抛。
    let err = pd
        .pauseSchedulersAndConfigWith(&ctx, &[scheduler.to_string()], None)
        .unwrap_err();
    assert_eq!(err.to_string(), "failed");

    // 失败路径后 Resume 应可调用；无存活 pause loop 时 send 被忽略。
    // Go spawns a goroutine to recv schedulerPauseCh when no pause loop is running.
    // Rust ResumeSchedulers send is ignored if the receiver was already dropped.
    pd.ResumeSchedulers(&ctx, &[scheduler.to_string()])
        .expect("resume after failed pause");

    // 与 Go TestScheduler 相同的暂停配置键集合。
    let cfg = HashMap::from([
        ("max-merge-region-keys".to_string(), Value::from(0)),
        ("max-snapshot".to_string(), Value::from(1)),
        (
            "enable-location-replacement".to_string(),
            Value::from(false),
        ),
        ("max-pending-peer-count".to_string(), Value::from(16_u64)),
    ]);
    let err = pd
        .pauseSchedulersAndConfigWith(&ctx, &[], Some(&cfg))
        .unwrap_err();
    // 配置更新失败必须包装为 ErrPDUpdateFailed，文案含 failed to update PD。
    assert!(Is(Some(&err), &astersql_br_pkg_errors::ErrPDUpdateFailed));
    assert!(
        err.to_string().starts_with("failed to update PD")
            || err.to_string().contains("failed to update PD"),
        "err={err}"
    );
    pd.ResumeSchedulers(&ctx, &[scheduler.to_string()])
        .expect("resume after config fail");

    // 清除注入错误后，暂停调度器加配置应成功。
    mock.set_err(None);

    pd.pauseSchedulersAndConfigWith(&ctx, &[scheduler.to_string()], Some(&cfg))
        .expect("pause schedulers and config");

    // Resume 唤醒后台刷新循环，避免测试悬挂。
    // pauseSchedulersAndConfigWith waits on schedulerPauseCh; resume wakes it.
    pd.ResumeSchedulers(&ctx, &[scheduler.to_string()])
        .expect("resume after successful pause");

    // 关闭控制器，释放 HTTP mock 与 pause channel。
    pd.Close();
}

// 校验 parseVersion：去引号、去 v 前缀、保留 pre-release。
/// Go `TestPDVersion`.
#[test]
fn test_pd_version() {
    // 模拟 PD HTTP 返回带引号与换行的版本串。
    let v = "\"v4.1.0-alpha1\"\n";
    let r = parseVersion(v);
    let expect_v = semver::Version::parse("4.1.0-alpha1").expect("semver");
    assert_eq!(expect_v.major, r.major);
    assert_eq!(expect_v.minor, r.minor);
    assert_eq!(expect_v.pre, r.pre);
}

// Forbidden 状态应被忽略（老 PD 无 ResetTS）；随后验证无错路径。
/// Go `TestPDResetTSCompatibility`.
#[test]
fn test_pd_reset_ts_compatibility() {
    let ctx = Context::Background();
    // 错误串需包含 Forbidden，与 pd.rs ResetTS 兼容分支一致。
    // http.StatusText(http.StatusForbidden) == "Forbidden"
    let mock =
        MockPDHTTPClient::with_err(Some("request pd http api failed with status: 'Forbidden'"));
    let pd = PdController::NewPdControllerWithPDClient(
        None,
        Arc::clone(&mock) as Arc<dyn PdHttpClient>,
        parseVersion("v0.0.0"),
    );

    // 兼容分支：Forbidden 不得导致测试失败。
    pd.ResetTS(&ctx, 123).expect("Forbidden ignored");

    mock.set_err(None);
    pd.ResetTS(&ctx, 123).expect("nil error ok");
}

// 按键范围暂停：后台按 TTL/3 刷新 label rule，cancel 后应清空过期表。
/// Go `TestPauseSchedulersByKeyRange`.
///
// Rust 用 trait mock 替代 httptest，断言点仍是 label create/refresh/delete。
/// Go uses httptest + pdhttp client; Rust mocks the same PD HTTP boundary via
/// `PdHttpClient`, asserting label-rule TTL create/refresh and PATCH delete.
#[test]
fn test_pause_schedulers_by_key_range() {
    // 短 TTL 加速等待；睡 3 倍 TTL 后 cancel，断言后台退出并清理。
    let ttl = Duration::from_secs(1);
    let mock = MockPDHTTPClient::with_err(None);

    let ctx = Context::Background();
    // 全范围键：覆盖从零到 0xff 的暂停区间。
    let start_key = vec![0, 0, 0, 0];
    let end_key = vec![0xff, 0xff, 0xff, 0xff];
    // 启动后台刷新；返回 done channel 供等待退出。
    let (done, _rule_id) = pause_scheduler_by_key_range_with_ttl(
        &ctx,
        Arc::clone(&mock) as Arc<dyn PdHttpClient>,
        &[[start_key, end_key]],
        ttl,
    )
    .expect("pause by key range");

    // 给刷新循环至少跑若干 tick 的时间。
    std::thread::sleep(ttl * 3);
    // 取消触发清理：PATCH 删除规则并关闭 done。
    ctx.cancel();
    done.expect("done channel")
        .recv_timeout(Duration::from_secs(5))
        .expect("background exit");

    assert!(
        // cancel 后 label_expires 必须清空，证明清理路径执行。
        mock.label_expires.lock().unwrap().is_empty(),
        "label expires should be cleared after cancel cleanup"
    );
}
