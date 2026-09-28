// Copyright 2026 AsterSQL.

// 本文件对 Go `pd.go`/`utils.go` 做契约级对齐测试，覆盖配置生成、placement、暂停生命周期。
// Mock 同时实现 PlacementHttpClient 与 PdHttpClient/PdClient，避免真实网络。
// 断言同时检查成功路径、错误包装类型与资源关闭幂等性。
// 与 pd_serial_test 分工：此处聚合多条契约，串行文件贴近 Go 单测命名。
//! Parity tests for `br/pkg/pdutil` vs Go sources (`pd.go`, `utils.go`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_br_pkg_errors::Is;
use astersql_errors::SharedError;
use serde_json::Value;

use crate::pd::{
    Context, DefaultExpectPDCfgGenerators, FetchPDVersion, LabelRule, LabelRulePatch, PdClient,
    PdController, PdHttpClient, Schedulers, StoreInfo, maxPendingPeerUnlimited, parseVersion,
    pause_scheduler_by_key_range_with_ttl, pauseConfigFalse, pauseConfigMulStores, pauseTimeout,
    zeroPauseConfig,
};
use crate::utils::{
    GetPlacementRules, PeerRoleType, PlacementHttpClient, Rule, SearchPlacementRule, Voter,
    encode_bytes, nop_undo, table_start_key_hex,
};

#[derive(Default)]
// placement 规则 HTTP 边界 mock：预设 status/body，并记录请求 URL。
struct MockPlacementHttp {
    // 模拟 HTTP 状态码（200/412/500 等）。
    status: u16,
    // 响应体；成功路径为 Rule JSON 数组。
    body: Vec<u8>,
    // 供断言 scheme（http/https）与 path 是否正确拼接。
    last_url: Mutex<String>,
}

// 记录 URL 后原样返回预设 status/body。
impl PlacementHttpClient for MockPlacementHttp {
    fn get_placement_rules(
        &self,
        _ctx: &Context,
        url: &str,
    ) -> Result<(u16, Vec<u8>), SharedError> {
        *self.last_url.lock().unwrap() = url.to_string();
        Ok((self.status, self.body.clone()))
    }
}

#[derive(Default)]
// PD HTTP 面 mock：可注入 pause/config/ResetTS 错误，并跟踪 paused/label_rules。
struct MockPdHttp {
    // GetPDVersion 返回值，供 FetchPDVersion/parseVersion 使用。
    version: String,
    // 集群版本字符串，独立于 PD binary 版本。
    cluster_version: String,
    // 当前调度器列表；RemoveSchedulers 会按 impact 集合过滤。
    schedulers: Mutex<Vec<String>>,
    // name → delay；delay=0 表示恢复。
    paused: Mutex<HashMap<String, i64>>,
    // 调度配置；SetConfig 同时写入带 schedule. 前缀与裸键。
    cfg: Mutex<HashMap<String, Value>>,
    // Close 标记，用于断言资源释放。
    closed: Mutex<bool>,
    // GetRegionCountByKeyRange 固定返回值。
    region_count: i32,
    // ResetTS 错误注入（含 Forbidden 兼容）。
    reset_ts_err: Mutex<Option<String>>,
    // 区域标签规则表，模拟按键范围暂停。
    label_rules: Mutex<HashMap<String, LabelRule>>,
    // SetConfig 失败注入。
    set_config_err: Mutex<Option<String>>,
    // 记录 SetConfig 调用，覆盖空配置也必须发送的 Go 副作用契约。
    set_config_calls: Mutex<Vec<(HashMap<String, Value>, Option<f64>)>>,
    // SetSchedulerDelay 失败注入。
    pause_err: Mutex<Option<String>>,
}

// 关键写路径会更新内部状态；读路径返回快照。
impl PdHttpClient for MockPdHttp {
    fn GetClusterVersion(&self, _ctx: &Context) -> Result<String, SharedError> {
        Ok(self.cluster_version.clone())
    }
    fn GetPDVersion(&self, _ctx: &Context) -> Result<String, SharedError> {
        Ok(self.version.clone())
    }
    fn GetRegionCountByKeyRange(
        &self,
        _ctx: &Context,
        _start: &[u8],
        _end: &[u8],
    ) -> Result<i32, SharedError> {
        // 区域计数透传预设值。
        Ok(self.region_count)
    }
    fn GetStore(&self, _ctx: &Context, store_id: u64) -> Result<StoreInfo, SharedError> {
        // store 地址用 store-{id} 便于辨认。
        Ok(StoreInfo {
            id: store_id,
            address: format!("store-{store_id}"),
        })
    }
    fn GetSchedulers(&self, _ctx: &Context) -> Result<Vec<String>, SharedError> {
        Ok(self.schedulers.lock().unwrap().clone())
    }
    fn GetScheduleConfig(&self, _ctx: &Context) -> Result<HashMap<String, Value>, SharedError> {
        Ok(self.cfg.lock().unwrap().clone())
    }
    fn SetConfig(
        &self,
        _ctx: &Context,
        cfg: &HashMap<String, Value>,
        _ttl_seconds: Option<f64>,
    ) -> Result<(), SharedError> {
        self.set_config_calls
            .lock()
            .unwrap()
            .push((cfg.clone(), _ttl_seconds));
        // 优先返回注入错误，再合并配置。
        if let Some(err) = self.set_config_err.lock().unwrap().as_ref() {
            return Err(astersql_errors::New(err.clone()));
        }
        // 兼容断言既查 schedule.xxx 也查裸键。
        // Store under both prefixed and raw keys for assertions.
        let mut guard = self.cfg.lock().unwrap();
        for (k, v) in cfg {
            guard.insert(k.clone(), v.clone());
            if let Some(raw) = k.strip_prefix("schedule.") {
                guard.insert(raw.to_string(), v.clone());
            }
        }
        Ok(())
    }
    fn SetSchedulerDelay(&self, _ctx: &Context, name: &str, delay: i64) -> Result<(), SharedError> {
        // pause 错误优先于状态更新。
        if let Some(err) = self.pause_err.lock().unwrap().as_ref() {
            return Err(astersql_errors::New(err.clone()));
        }
        let mut paused = self.paused.lock().unwrap();
        // delay=0 表示停止暂停，从 paused 表删除。
        if delay == 0 {
            paused.remove(name);
        } else {
            paused.insert(name.to_string(), delay);
        }
        Ok(())
    }
    fn GetRegionLabelRulesByIDs(
        &self,
        _ctx: &Context,
        ids: &[String],
    ) -> Result<Vec<LabelRule>, SharedError> {
        let rules = self.label_rules.lock().unwrap();
        // 按 id 过滤已存在规则。
        Ok(ids.iter().filter_map(|id| rules.get(id).cloned()).collect())
    }
    fn PatchRegionLabelRules(
        &self,
        _ctx: &Context,
        patch: &LabelRulePatch,
    ) -> Result<(), SharedError> {
        let mut rules = self.label_rules.lock().unwrap();
        // 先删后设，对齐 PD patch 语义。
        for id in &patch.DeleteRules {
            rules.remove(id);
        }
        for r in &patch.SetRules {
            rules.insert(r.ID.clone(), r.clone());
        }
        Ok(())
    }
    fn SetRegionLabelRule(&self, _ctx: &Context, rule: &LabelRule) -> Result<(), SharedError> {
        // SetRegionLabelRule：按 ID 覆盖写入。
        self.label_rules
            .lock()
            .unwrap()
            .insert(rule.ID.clone(), rule.clone());
        Ok(())
    }
    // 固定返回 42，仅验证调用可达。
    fn GetMinResolvedTSByStoresIDs(
        &self,
        _ctx: &Context,
        _store_ids: Option<&[u64]>,
    ) -> Result<u64, SharedError> {
        Ok(42)
    }
    // 恢复分配 ID：本契约测试不注入失败。
    fn ResetBaseAllocID(&self, _ctx: &Context, _id: u64) -> Result<(), SharedError> {
        Ok(())
    }
    fn ResetTS(&self, _ctx: &Context, _ts: u64, _force: bool) -> Result<(), SharedError> {
        // ResetTS 错误注入点。
        if let Some(err) = self.reset_ts_err.lock().unwrap().as_ref() {
            return Err(astersql_errors::New(err.clone()));
        }
        Ok(())
    }
    // 快照恢复标记：成功空操作。
    fn SetSnapshotRecoveringMark(&self, _ctx: &Context) -> Result<(), SharedError> {
        Ok(())
    }
    // 删除恢复标记：成功空操作。
    fn DeleteSnapshotRecoveringMark(&self, _ctx: &Context) -> Result<(), SharedError> {
        Ok(())
    }
    // 标记 closed，供后续断言。
    fn Close(&self) {
        *self.closed.lock().unwrap() = true;
    }
}

#[test]
fn empty_schedule_config_is_still_sent_for_pause_and_restore() {
    let ctx = Context::new();
    let http = Arc::new(MockPdHttp::default());
    let pd_cli = Arc::new(MockPdClient {
        follower: Mutex::new(false),
        stores: Vec::new(),
        closed: Mutex::new(false),
    });
    let ctl = PdController::NewPdControllerWithPDClient(
        Some(pd_cli as Arc<dyn PdClient>),
        Arc::clone(&http) as Arc<dyn PdHttpClient>,
        parseVersion("v6.5.0"),
    );
    let generators = HashMap::from([(
        "missing-config".to_string(),
        Arc::new(zeroPauseConfig) as crate::pd::PauseConfigGenerator,
    )]);

    let (origin, _, result) = ctl.RemoveSchedulersWithConfigGenerator(&ctx, &generators);
    result.expect("pause with an empty generated config");
    {
        let calls = http.set_config_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "Go sends a non-nil empty pause config");
        assert!(calls[0].0.is_empty());
        assert_eq!(calls[0].1, Some(pauseTimeout.as_secs_f64()));
    }

    ctl.GenRestoreSchedulerFunc(origin, generators)(Context::new())
        .expect("restore an empty saved config");
    let calls = http.set_config_calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "Go sends the empty restore config too");
    assert!(calls[1].0.is_empty());
    assert_eq!(calls[1].1, Some(0.0));
}

#[test]
fn restore_config_error_keeps_pd_update_classification() {
    let http = Arc::new(MockPdHttp {
        set_config_err: Mutex::new(Some("restore failed".to_string())),
        ..Default::default()
    });
    let ctl = PdController::NewPdControllerWithPDClient(
        None,
        Arc::clone(&http) as Arc<dyn PdHttpClient>,
        parseVersion("v6.5.0"),
    );
    let config = crate::pd::ClusterConfig {
        ScheduleCfg: HashMap::from([("merge-schedule-limit".to_string(), Value::from(8))]),
        ..Default::default()
    };

    let err = ctl.MakeUndoFunctionByConfig(config)(Context::new())
        .expect_err("restore SetConfig must fail");
    assert!(Is(Some(&err), &astersql_br_pkg_errors::ErrPDUpdateFailed));
    assert!(err.to_string().contains("fail to update PD merge config"));
    assert!(
        err.to_string()
            .contains("failed to update PD schedule config")
    );
}

// gRPC 风格 PD client mock：提供 store 列表与 follower handle。
struct MockPdClient {
    // UpdateFollowerHandle 写入的开关状态。
    follower: Mutex<bool>,
    // GetAllStores 返回；影响 pauseConfigMulStores 的 store 计数。
    stores: Vec<StoreInfo>,
    closed: Mutex<bool>,
}

// 最小实现，足以驱动 RemoveSchedulers 的 store_count 逻辑。
impl PdClient for MockPdClient {
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<StoreInfo>, SharedError> {
        Ok(self.stores.clone())
    }
    // 记录 follower 开关，无其他副作用。
    fn UpdateFollowerHandle(&self, enable: bool) -> Result<(), SharedError> {
        *self.follower.lock().unwrap() = enable;
        Ok(())
    }
    fn Close(&self) {
        *self.closed.lock().unwrap() = true;
    }
}

#[test]
// 聚合契约：配置生成器、版本解析、placement、暂停/恢复、Close 幂等。
fn go_rust_public_contract_matches() {
    // 配置生成器：zero / mulStores（封顶 40）/ false / pending 上限。
    // normal: pause config generators match Go
    // zeroPauseConfig 忽略输入，恒为 0。
    assert_eq!(zeroPauseConfig(3, &Value::from(9)), Value::from(0));
    assert_eq!(
        pauseConfigMulStores(3, &Value::from(10.0)),
        Value::from(30.0)
    );
    assert_eq!(
        pauseConfigMulStores(10, &Value::from(10.0)),
        Value::from(40.0)
    ); // capped at 40
    // store*raw 超过 40 时封顶，避免把调度打满。
    assert_eq!(
        pauseConfigFalse(1, &Value::Null),
        Value::String("false".into())
    );
    // pending peer 上限对齐 Go math.MaxInt32。
    assert_eq!(maxPendingPeerUnlimited, i32::MAX as u64);

    // 默认 generator 集合必须包含 merge 与 max-pending-peer-count。
    let gens = DefaultExpectPDCfgGenerators();
    assert!(gens.contains_key("merge-schedule-limit"));
    assert!(gens.contains_key("max-pending-peer-count"));
    assert_eq!(
        gens["max-pending-peer-count"](0, &Value::Null),
        Value::from(maxPendingPeerUnlimited)
    );

    // 边界：空白、非法版本回落 0.0.0；合法版本保留 major/minor/pre。
    // boundary: parseVersion trim / strip v / invalid fallback (TestPDVersion)
    let r = parseVersion("\"v4.1.0-alpha1\"\n");
    assert_eq!(r.major, 4);
    assert_eq!(r.minor, 1);
    assert_eq!(r.pre.as_str(), "alpha1");
    assert_eq!(parseVersion(" v6.1.0 "), semver::Version::new(6, 1, 0));
    // 非法版本回落 0.0.0，与 Go 一致。
    assert_eq!(parseVersion("not-a-version"), semver::Version::new(0, 0, 0));

    // 正常：按 table id + Voter 命中；Leader 角色不得误匹配。
    // normal: SearchPlacementRule matches table id + role
    let tid = 42i64;
    let rule = Rule {
        StartKeyHex: table_start_key_hex(tid),
        Role: Voter,
        ID: "r1".into(),
        ..Default::default()
    };
    // 夹杂非法 StartKey 与其他 table，验证跳过与精确命中。
    let rules = vec![
        Rule {
            StartKeyHex: "zz".into(),
            Role: Voter,
            ..Default::default()
        },
        rule.clone(),
        Rule {
            StartKeyHex: table_start_key_hex(99),
            Role: Voter,
            ..Default::default()
        },
    ];
    let found = SearchPlacementRule(tid, &rules, Voter).expect("found");
    assert_eq!(found.ID, "r1");
    // 角色不匹配时必须返回 None。
    assert!(SearchPlacementRule(tid, &rules, PeerRoleType::Leader).is_none());

    // memcomparable：空串与短字节的编码常量应对齐 TiDB codec。
    // encode/decode roundtrip for empty / short bytes (memcomparable)
    assert_eq!(encode_bytes(&[]), vec![0, 0, 0, 0, 0, 0, 0, 0, 247]);
    assert_eq!(encode_bytes(&[1, 2, 3]), vec![1, 2, 3, 0, 0, 0, 0, 0, 250]);

    // 错误：非 200 必须标注 ErrPDInvalidResponse，且 URL 为 http。
    // error: GetPlacementRules non-OK status annotates ErrPDInvalidResponse
    let cli = MockPlacementHttp {
        status: 500,
        body: b"boom".to_vec(),
        ..Default::default()
    };
    let err = GetPlacementRules(&Context::new(), "127.0.0.1:2379", false, &cli).unwrap_err();
    assert!(Is(
        Some(&err),
        &astersql_br_pkg_errors::ErrPDInvalidResponse
    ));
    // URL 必须以 http://127.0.0.1:2379 开头（非 TLS）。
    assert!(
        cli.last_url
            .lock()
            .unwrap()
            .starts_with("http://127.0.0.1:2379")
    );

    // 边界：412 表示 placement 未启用，返回空列表且使用 https。
    // boundary: 412 → empty rules
    let cli412 = MockPlacementHttp {
        status: 412,
        body: Vec::new(),
        ..Default::default()
    };
    let empty = GetPlacementRules(&Context::new(), "pd", true, &cli412).unwrap();
    assert!(empty.is_empty());
    // use_tls=true 时 scheme 为 https。
    assert!(cli412.last_url.lock().unwrap().starts_with("https://"));

    // 正常：200 + JSON 反序列化得到规则。
    // normal JSON OK path
    let body = serde_json::to_vec(&[rule]).unwrap();
    let cli_ok = MockPlacementHttp {
        status: 200,
        body,
        ..Default::default()
    };
    let got = GetPlacementRules(&Context::new(), "pd", false, &cli_ok).unwrap();
    assert_eq!(got.len(), 1);
    // 反序列化后 ID 与输入 rule 一致。
    assert_eq!(got[0].ID, "r1");

    // 错误：首个 SetSchedulerDelay 失败应原样返回。
    // error: pauseSchedulersAndConfigWith fails on first SetSchedulerDelay (TestScheduler)
    let http_fail = Arc::new(MockPdHttp {
        pause_err: Mutex::new(Some("failed".into())),
        ..Default::default()
    });
    let ctl_fail = PdController::NewPdControllerWithPDClient(
        None,
        Arc::clone(&http_fail) as Arc<dyn PdHttpClient>,
        parseVersion("v6.5.0"),
    );
    let err = ctl_fail
        .pauseSchedulersAndConfigWith(&Context::new(), &["balance-leader-scheduler".into()], None)
        .unwrap_err();
    // 调度器 pause 失败不包装，保持底层文案。
    assert_eq!(err.to_string(), "failed");

    // 错误：配置暂停失败包装 ErrPDUpdateFailed。
    // error: config pause annotates ErrPDUpdateFailed
    *http_fail.pause_err.lock().unwrap() = None;
    *http_fail.set_config_err.lock().unwrap() = Some("cfg boom".into());
    let cfg = HashMap::from([("max-merge-region-keys".into(), Value::from(0))]);
    let err = ctl_fail
        .pauseSchedulersAndConfigWith(&Context::new(), &[], Some(&cfg))
        .unwrap_err();
    assert!(Is(Some(&err), &astersql_br_pkg_errors::ErrPDUpdateFailed));
    // 配置失败文案前缀对齐 Go Annotate。
    assert!(err.to_string().contains("failed to update PD"));

    // 正常：Forbidden 忽略；清除错误后 ResetTS 成功。
    // normal: ResetTS Forbidden is ignored (TestPDResetTSCompatibility)
    *http_fail.reset_ts_err.lock().unwrap() =
        Some("request pd http api failed with status: 'Forbidden'".into());
    ctl_fail.ResetTS(&Context::new(), 123).unwrap();
    *http_fail.reset_ts_err.lock().unwrap() = None;
    ctl_fail.ResetTS(&Context::new(), 123).unwrap();

    // 资源生命周期：RemoveSchedulers → undo → 按键范围 pause → Close。
    // resource: PdController pause/resume/close lifecycle
    // 构造含 impact 调度器与自定义调度器，验证只暂停 impact 集合。
    let http = Arc::new(MockPdHttp {
        version: "v6.5.0".into(),
        cluster_version: "6.5.0".into(),
        schedulers: Mutex::new(vec![
            "balance-leader-scheduler".into(),
            "custom-scheduler".into(),
        ]),
        cfg: Mutex::new(HashMap::from([
            ("merge-schedule-limit".into(), Value::from(8)),
            ("leader-schedule-limit".into(), Value::from(4.0)),
        ])),
        region_count: 7,
        ..Default::default()
    });
    // 两个 store，驱动 MulStores 相关配置生成。
    let pd_cli = Arc::new(MockPdClient {
        follower: Mutex::new(false),
        stores: vec![
            StoreInfo {
                id: 1,
                address: "s1".into(),
            },
            StoreInfo {
                id: 2,
                address: "s2".into(),
            },
        ],
        closed: Mutex::new(false),
    });
    let ctl = PdController::NewPdControllerWithPDClient(
        Some(Arc::clone(&pd_cli) as Arc<dyn PdClient>),
        Arc::clone(&http) as Arc<dyn PdHttpClient>,
        parseVersion("v6.5.0"),
    );
    // v6.5.0 应开启 pause config 与 key-range label TTL。
    assert!(ctl.isPauseConfigEnabled());
    assert!(ctl.CanPauseSchedulerByKeyRange());
    assert_eq!(ctl.ttlOfPausing(), pauseTimeout);
    assert_eq!(ctl.GetRegionCount(&Context::new(), b"a", b"b").unwrap(), 7);
    assert_eq!(
        FetchPDVersion(&Context::new(), http.as_ref()).unwrap(),
        semver::Version::new(6, 5, 0)
    );

    // impact 集合不含 custom-scheduler。
    let impact = Schedulers();
    assert!(impact.contains("balance-leader-scheduler"));
    assert!(!impact.contains("custom-scheduler"));

    // 暂停后应仅 balance-leader 出现在 paused。
    let undo = ctl.RemoveSchedulers(&Context::new()).unwrap();
    assert!(
        http.paused
            .lock()
            .unwrap()
            .contains_key("balance-leader-scheduler")
    );
    assert!(!http.paused.lock().unwrap().contains_key("custom-scheduler"));

    // undo 恢复后 paused 中不应再有该调度器。
    undo(Context::new()).expect("undo ok");
    assert!(
        !http
            .paused
            .lock()
            .unwrap()
            .contains_key("balance-leader-scheduler")
    );

    // 按键范围：创建 rule → cancel → done 收到后规则表清空。
    // Pause by key range: set rule, cancel, wait done cleans up.
    let ctx = Context::new();
    let (done, rule_id) = pause_scheduler_by_key_range_with_ttl(
        &ctx,
        Arc::clone(&http) as Arc<dyn PdHttpClient>,
        &[[vec![0, 0, 0, 0], vec![0xff, 0xff, 0xff, 0xff]]],
        Duration::from_millis(30),
    )
    .unwrap();
    assert!(!rule_id.is_empty());
    assert!(http.label_rules.lock().unwrap().contains_key(&rule_id));
    ctx.cancel();
    done.unwrap().recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!http.label_rules.lock().unwrap().contains_key(&rule_id));

    // follower handle 开关应落到 PdClient mock。
    ctl.SetFollowerHandle(true).unwrap();
    assert!(*pd_cli.follower.lock().unwrap());
    // Close 应同时关闭 HTTP 与 PD client。
    ctl.Close();
    assert!(*http.closed.lock().unwrap());
    assert!(*pd_cli.closed.lock().unwrap());
    // 第二次 Close 必须幂等，不 panic。
    ctl.Close(); // idempotent

    // 触及 Nop undo，确保零值回滚可调用。
    let _ = nop_undo();
}
