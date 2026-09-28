// Copyright 2026 AsterSQL.

//! streamhelper 包级公开契约的 Go/Rust 对齐测试。
//!
//! 用内存假集群与 MemEtcd 串联 models 路径、region 一致性、前缀扫描、
//! 检查点解析、PauseV2、MetaDataClient/AdvancerExt、StoreCheckpoints.merge、
//! flush/resolve-lock 辅助，以及 CheckpointAdvancer 一次 tick 推进全局检查点。
//! 只锁定行为契约，不引入网络或真实 PD/TiKV。
//!
//! 设计意图：
//! - 假集群一次实现多 trait，避免为每个子系统单独搭脚手架。
//! - FixedClient / MemSource 隔离 RPC 与 etcd，使断言可确定复现。
//! - 覆盖面按 Go 公开符号分组，便于回归时对照 `models.go` 等源文件。
//! - 不启动真实 PD/TiKV；失败即表示 Rust 契约漂移，而非环境抖动。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::advancer_cliext::{AdvancerExt, TaskEvent};
use crate::advancer_env::{Env, LogBackupFlushIntervalGetter, RegionLockResolver, StreamMeta};
use crate::export_test::{CheckpointAdvancerTestExt, NewCheckpointAdvancerForTest};
use crate::prefix_scanner::Source;
use crate::regioniter::TiKVClusterMeta;
use crate::stubs::{
    Entry, EtcdKV, GetLastFlushTSOfRegionRequest, GetLastFlushTSOfRegionResponse, KeyRange,
    LogBackupClient, LogBackupService, RegionCheckpoint, RegionIdentity,
};
use crate::{
    CheckPointsOf, CheckRegionConsistency, Checkpoint, CheckpointAdvancer, CheckpointType,
    EventType, GlobalCheckpointOf, IterateRegion, LastErrorPrefixOf, MemEtcd, MetaDataClient,
    NewCheckpointAdvancer, NewClusterCollector, NewLocalPauseV2, NewMetaDataClient, NewSubscriber,
    NewTaskInfo, OwnerManagerPath, OwnerManagerPrompt, ParseCheckpoint, Pause, PausePayload,
    PauseV2, PauseWithErrorSeverity, PauseWithMessage, Peer, PrefixNextKey, PrefixOfTask,
    RFC3339Time, RangesOf, Region, RegionWithLeader, SeverityError, StorageBackend,
    StorageCheckpointOf, Store, StoreCheckpoints, StreamBackupError, StreamBackupTaskInfo, TaskOf,
    encodeUint64, isScanLockLockedError, lowerResolveLockMaxVersion, parseGlobalCheckpointValue,
    parseLogBackupFlushIntervalFromConfig, resolveLockRetryLowerBound, resolveLockTargetUpperBound,
    scanPrefix,
};

/// 内存假集群：同时实现 region 扫描、日志备份客户端、元数据与锁解析接口。
/// 字段均用 Mutex 包裹，以模拟并发 tick / 订阅更新时的共享状态。
/// 全局检查点 map 只升不降，对应 Go V3 Upload 语义。
#[derive(Default)]
struct FakeCluster {
    /// 有序/无序均可；扫描按键区间过滤。
    regions: Mutex<Vec<RegionWithLeader>>,
    /// 拓扑中的 store 列表，供订阅与 collector 使用。
    stores: Mutex<Vec<Store>>,
    /// storeID → 固定返回检查点的客户端。
    clients: Mutex<HashMap<u64, Arc<dyn LogBackupClient>>>,
    /// 任务名 → 已上传的全局检查点（单调不减）。
    global: Mutex<HashMap<String, u64>>,
    /// `GetLogBackupFlushInterval` 返回值，驱动 resolve-lock 间隔。
    flush: Duration,
}

impl TiKVClusterMeta for FakeCluster {
    /// 按 `[key, endKey)` 过滤 region；空 endKey 视为 +∞。
    /// 不保证返回顺序与 PD 完全一致，但足以驱动一致性校验与分页。
    fn RegionScan(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionWithLeader>, String> {
        let all = self.regions.lock().unwrap().clone();
        let mut out = Vec::new();
        for r in all {
            // 起点已越过扫描上界则跳过。
            if !endKey.is_empty() && r.Region.StartKey.as_slice() >= endKey {
                continue;
            }
            // 终点不晚于扫描起点则无重叠。
            if !r.Region.EndKey.is_empty() && r.Region.EndKey.as_slice() <= key {
                continue;
            }
            out.push(r);
            if out.len() as i32 >= limit {
                break;
            }
        }
        Ok(out)
    }
    fn Stores(&self) -> Result<Vec<Store>, String> {
        Ok(self.stores.lock().unwrap().clone())
    }
    /// 测试中直接回显请求的阻塞 TS。
    fn BlockGCUntil(&self, at: u64) -> Result<u64, String> {
        Ok(at)
    }
    fn UnblockGC(&self) -> Result<(), String> {
        Ok(())
    }
    /// 固定伪造当前 TS，避免依赖墙钟。
    fn FetchCurrentTS(&self) -> Result<u64, String> {
        Ok(1 << 18)
    }
}

impl LogBackupService for FakeCluster {
    /// 按 store 查客户端；缺失则报错，模拟未就绪拓扑。
    fn GetLogBackupClient(&self, storeID: u64) -> Result<Arc<dyn LogBackupClient>, String> {
        self.clients
            .lock()
            .unwrap()
            .get(&storeID)
            .cloned()
            .ok_or_else(|| format!("no client for {storeID}"))
    }
    fn ClearCache(&self, _storeID: u64) -> Result<(), String> {
        Ok(())
    }
}

impl StreamMeta for FakeCluster {
    /// 任务监听入口；测试不注入事件，直接成功。
    fn Begin(&self, _ch: &mut Vec<TaskEvent>) -> Result<(), String> {
        Ok(())
    }
    /// 全局检查点只升不降，对齐 V3 上传语义。
    /// 较低值静默忽略，避免 advancer 回退。
    fn UploadV3GlobalCheckpointForTask(
        &self,
        taskName: &str,
        checkpoint: u64,
    ) -> Result<(), String> {
        let mut g = self.global.lock().unwrap();
        let old = g.get(taskName).copied().unwrap_or(0);
        if checkpoint >= old {
            g.insert(taskName.to_string(), checkpoint);
        }
        Ok(())
    }
    fn GetGlobalCheckpointForTask(&self, taskName: &str) -> Result<u64, String> {
        Ok(self
            .global
            .lock()
            .unwrap()
            .get(taskName)
            .copied()
            .unwrap_or(0))
    }
    fn ClearV3GlobalCheckpointForTask(&self, taskName: &str) -> Result<(), String> {
        self.global.lock().unwrap().remove(taskName);
        Ok(())
    }
    fn PauseTask(&self, _taskName: &str) -> Result<(), String> {
        Ok(())
    }
}

impl RegionLockResolver for FakeCluster {
    /// 空实现：本测试不验证真实 resolve-lock RPC。
    fn ResolveLocksForRange(
        &self,
        _maxVersion: u64,
        _startKey: &[u8],
        _endKey: &[u8],
    ) -> Result<(), String> {
        Ok(())
    }
}

impl LogBackupFlushIntervalGetter for FakeCluster {
    fn GetLogBackupFlushInterval(&self) -> Result<Duration, String> {
        Ok(self.flush)
    }
}

/// 固定返回预设 region 检查点列表的客户端替身。
/// 忽略请求内容，始终回放构造时注入的 `cps`。
struct FixedClient {
    /// 预设的 region 级 flush TS 列表。
    cps: Vec<RegionCheckpoint>,
}

impl LogBackupClient for FixedClient {
    fn GetLastFlushTSOfRegion(
        &self,
        _req: &GetLastFlushTSOfRegionRequest,
    ) -> Result<GetLastFlushTSOfRegionResponse, String> {
        Ok(GetLastFlushTSOfRegionResponse {
            Checkpoints: self.cps.clone(),
        })
    }
}

/// 内存键值源，供前缀扫描分页断言。
/// 数据可无序存放，`Scan` 内会排序以保证翻页确定性。
struct MemSource {
    /// 全量键值；扫描时再按区间过滤。
    kvs: Vec<Entry>,
}

impl Source for MemSource {
    /// 半开区间过滤后排序截断；`more` 表示还有未返回条目。
    fn Scan(&self, from: &[u8], to: &[u8], limit: i32) -> Result<(Vec<Entry>, bool), String> {
        let mut matched: Vec<_> = self
            .kvs
            .iter()
            .filter(|e| e.Key.as_slice() >= from && e.Key.as_slice() < to)
            .cloned()
            .collect();
        matched.sort_by(|a, b| a.Key.cmp(&b.Key));
        let more = matched.len() as i32 > limit;
        matched.truncate(limit as usize);
        Ok((matched, more))
    }
}

#[test]
fn go_rust_public_contract_matches() {
    // ========== 分段 1：models 路径与 TaskInfo.Check ==========
    // models：路径布局与 TaskInfo.Check 约束必须与 Go 一致。
    // 路径字符串是 etcd watch/prefix scan 的契约，改动会破坏跨组件兼容。
    // models paths + TaskInfo.Check
    assert_eq!(PrefixOfTask(), "/tidb/br-stream/info/");
    assert_eq!(TaskOf("t1"), "/tidb/br-stream/info/t1");
    // ranges 尾斜杠防止扫到同前缀任务。
    assert_eq!(RangesOf("t1"), "/tidb/br-stream/ranges/t1/");
    // checkpoint 前缀同样要求规范化尾斜杠。
    assert_eq!(CheckPointsOf("t1"), "/tidb/br-stream/checkpoint/t1/");
    // central_global 后缀名与 Go 常量字节级一致。
    assert_eq!(
        GlobalCheckpointOf("t1"),
        "/tidb/br-stream/checkpoint/t1/central_global"
    );
    assert_eq!(Pause("t1"), "/tidb/br-stream/pause/t1");
    // 大端编码：低位字节落在末两字节。
    // 与 Go encoding/binary.BigEndian 布局对齐。
    assert_eq!(encodeUint64(0x0102), {
        let mut v = vec![0u8; 8];
        v[6] = 1;
        v[7] = 2;
        v
    });

    // 合法名 + 过滤 + 存储应通过；含连字符名应失败；缺过滤应失败。
    // Check 错误文案保持英文，与 Go 侧日志/CLI 提示一致。
    let ok = NewTaskInfo("ok_task")
        .WithTableFilter(&["*.*"])
        .ToStorage(StorageBackend {
            Uri: "s3://b".into(),
        })
        .Check()
        .unwrap();
    assert_eq!(ok.PBInfo.Name, "ok_task");
    // 连字符不在 taskNameRe 白名单内。
    assert!(
        NewTaskInfo("bad-name")
            .WithTableFilter(&["*.*"])
            .ToStorage(StorageBackend {
                Uri: "s3://b".into()
            })
            .Check()
            .is_err()
    );
    // 空 TableFilter 必须拒绝，避免误备份全空过滤语义。
    assert!(
        NewTaskInfo("ok")
            .ToStorage(StorageBackend {
                Uri: "s3://b".into()
            })
            .Check()
            .is_err()
    );

    // ========== 分段 2：region 一致性 ==========
    // region 一致性：连续覆盖成功；空列表/起点空洞失败。
    // region consistency
    let regions = vec![
        RegionWithLeader {
            Region: Region {
                Id: 1,
                StartKey: b"a".to_vec(),
                EndKey: b"m".to_vec(),
                ..Default::default()
            },
            Leader: Peer { Id: 1, StoreId: 1 },
        },
        RegionWithLeader {
            Region: Region {
                Id: 2,
                StartKey: b"m".to_vec(),
                EndKey: b"z".to_vec(),
                ..Default::default()
            },
            Leader: Peer { Id: 2, StoreId: 1 },
        },
    ];
    // 两段首尾相接，覆盖 [a,z)。
    CheckRegionConsistency(b"a", b"z", &regions).unwrap();
    // 空扫描结果一律错误。
    assert!(CheckRegionConsistency(b"a", b"z", &[]).is_err());
    // 请求起点早于首 region → 左侧空洞。
    assert!(CheckRegionConsistency(b"`", b"z", &regions).is_err());

    // ========== 分段 3：前缀扫描 ==========
    // 前缀扫描：PrefixNextKey 进位；分页只收集匹配前缀的键。
    // prefix scanner
    assert_eq!(PrefixNextKey(b"abc"), b"abd".to_vec());
    let src = MemSource {
        kvs: vec![
            Entry {
                Key: b"p/1".to_vec(),
                Value: b"a".to_vec(),
            },
            Entry {
                Key: b"p/2".to_vec(),
                Value: b"b".to_vec(),
            },
            Entry {
                Key: b"q/1".to_vec(),
                Value: b"c".to_vec(),
            },
        ],
    };
    // page size=1 强制翻页，仍应只得 p/ 下两条。
    // q/ 前缀外键不得泄漏进结果。
    let mut scan = scanPrefix(&src, "p/");
    let all = scan.AllPages(1).unwrap();
    assert_eq!(all.len(), 2);
    assert!(scan.Done());

    // ========== 分段 4：检查点解析 ==========
    // ParseCheckpoint：store/global 键形态与短 value 错误路径。
    // ParseCheckpoint
    let task = "t1";
    // store 键形如 .../checkpoint/t1/store/<id>。
    let store_key = format!("{}store/9", CheckPointsOf(task));
    let cp = ParseCheckpoint(task, store_key.as_bytes(), &encodeUint64(99)).unwrap();
    assert_eq!(cp.ID, 9);
    assert_eq!(cp.TS, 99);
    assert_eq!(cp.Type(), CheckpointType::Store);
    let gkey = GlobalCheckpointOf(task);
    let gcp = ParseCheckpoint(task, gkey.as_bytes(), &encodeUint64(7)).unwrap();
    assert!(gcp.IsGlobal);
    assert_eq!(gcp.Type(), CheckpointType::Global);
    // 非法键与过短 value 都应报错。
    assert!(ParseCheckpoint(task, b"bad", &encodeUint64(1)).is_err());
    assert!(ParseCheckpoint(task, store_key.as_bytes(), b"short").is_err());

    // ========== 分段 5：PauseV2 payload ==========
    // PauseV2：文本消息与流错误两种 payload。
    // PauseV2 payload
    let mut pause = NewLocalPauseV2();
    pause.SetTextMessage("manual");
    match pause.GetPayload().unwrap() {
        crate::client::PausePayload::Text(t) => assert_eq!(t, "manual"),
        _ => panic!("expected text"),
    }
    // 流错误路径保留 ErrorCode，供 CLI/UI 展示。
    let mut pause2 = NewLocalPauseV2();
    pause2
        .SetBakcupStreamError(&StreamBackupError {
            ErrorCode: "E1".into(),
            ErrorMessage: "boom".into(),
        })
        .unwrap();
    match pause2.GetPayload().unwrap() {
        crate::client::PausePayload::StreamErr(e) => assert_eq!(e.ErrorCode, "E1"),
        _ => panic!("expected stream err"),
    }

    // ========== 分段 6：元数据与全局检查点 ==========
    // MetaDataClient：Put/Get/Pause/Resume；AdvancerExt 全局检查点单调上传。
    // MetaDataClient + AdvancerExt global checkpoint
    let kv = Arc::new(MemEtcd::new());
    let meta = NewMetaDataClient(kv.clone());
    let task_info = NewTaskInfo("demo")
        .WithTableFilter(&["*.*"])
        .ToStorage(StorageBackend {
            Uri: "noop://".into(),
        })
        .WithRange(b"a", b"z");
    meta.PutTask(&task_info).unwrap();
    let got = meta.GetTask("demo").unwrap();
    assert_eq!(got.Info.Name, "demo");
    // 暂停后 GetTaskWithPauseStatus 第二返回值应为 true。
    meta.PauseTask("demo", Vec::new()).unwrap();
    let (_, paused) = meta.GetTaskWithPauseStatus("demo").unwrap();
    assert!(paused);
    meta.ResumeTask("demo").unwrap();

    let ext = AdvancerExt { meta: meta.clone() };
    // 全局检查点 value 必须是 8 字节大端。
    assert_eq!(parseGlobalCheckpointValue(&encodeUint64(42)).unwrap(), 42);
    assert!(parseGlobalCheckpointValue(b"x").is_err());
    ext.UploadV3GlobalCheckpointForTask("demo", 100).unwrap();
    assert_eq!(ext.GetGlobalCheckpointForTask("demo").unwrap(), 100);
    // 更低值应被跳过，保持 100。
    ext.UploadV3GlobalCheckpointForTask("demo", 50).unwrap(); // skip lower
    assert_eq!(ext.GetGlobalCheckpointForTask("demo").unwrap(), 100);
    // Clear 后读回应为 0。
    ext.ClearV3GlobalCheckpointForTask("demo").unwrap();
    assert_eq!(ext.GetGlobalCheckpointForTask("demo").unwrap(), 0);

    // ========== 分段 7：StoreCheckpoints.merge ==========
    // StoreCheckpoints.merge：取较小检查点并累计失败子区间。
    // 较小 TS 代表更保守的安全水位。
    // StoreCheckpoints.merge
    let mut sc = StoreCheckpoints {
        HasCheckpoint: true,
        Checkpoint: 20,
        FailureSubRanges: vec![],
    };
    sc.merge(StoreCheckpoints {
        HasCheckpoint: true,
        Checkpoint: 10,
        FailureSubRanges: vec![KeyRange {
            StartKey: b"a".to_vec(),
            EndKey: b"b".to_vec(),
        }],
    });
    assert_eq!(sc.Checkpoint, 10);
    assert_eq!(sc.FailureSubRanges.len(), 1);

    // ========== 分段 8：flush / resolve-lock 辅助 ==========
    // flush 间隔解析与 resolve-lock 上下界/错误识别辅助。
    // flush interval parse + resolve lock helpers
    let d = parseLogBackupFlushIntervalFromConfig(br#"{"log-backup":{"max-flush-interval":"3s"}}"#)
        .unwrap();
    assert_eq!(d, Duration::from_secs(3));
    // 缺字段配置应失败，避免静默用零间隔。
    assert!(parseLogBackupFlushIntervalFromConfig(br#"{}"#).is_err());
    // 相同入参应幂等。
    assert_eq!(
        resolveLockTargetUpperBound(1, Duration::ZERO, 100),
        resolveLockTargetUpperBound(1, Duration::ZERO, 100)
    );
    let (lb, ok) = resolveLockRetryLowerBound(1, u64::MAX);
    assert!(ok && lb > 1);
    // 文案含 key is locked 才视为可重试锁冲突。
    assert!(isScanLockLockedError(
        "unexpected scanlock error: key is locked"
    ));
    assert!(!isScanLockLockedError("other"));
    // 在 [10,100] 间下调 maxVersion（中点语义）。
    let (nv, ok2) = lowerResolveLockMaxVersion(100, 10);
    assert!(ok2 && nv == 55);

    // ========== 分段 9：advancer tick 端到端 ==========
    // collector + advancer：挂任务后 tick 应把 store 检查点 88 推到全局。
    // 拓扑：单 region、单 store=7、客户端返回 Checkpoint=88。
    // collector + advancer tick path
    let cluster = Arc::new(FakeCluster {
        flush: Duration::from_secs(4),
        ..Default::default()
    });
    cluster.regions.lock().unwrap().push(RegionWithLeader {
        Region: Region {
            Id: 1,
            StartKey: b"a".to_vec(),
            EndKey: b"z".to_vec(),
            ..Default::default()
        },
        Leader: Peer { Id: 11, StoreId: 7 },
    });
    cluster
        .stores
        .lock()
        .unwrap()
        .push(Store { ID: 7, BootAt: 1 });
    // store 7 的客户端固定回报 region1@88。
    cluster.clients.lock().unwrap().insert(
        7,
        Arc::new(FixedClient {
            cps: vec![RegionCheckpoint {
                Region: RegionIdentity {
                    Id: 1,
                    EpochVersion: 0,
                },
                Checkpoint: 88,
                Err: None,
            }],
        }),
    );

    let adv = NewCheckpointAdvancer(cluster.clone());
    assert_eq!(adv.Name(), "LogBackup::Advancer");
    assert!(!adv.HasTask());
    // 无任务时 tick 应为空操作成功。
    adv.OnTick().unwrap(); // no task -> ok
    // 设置任务范围 [a,z) 后进入可推进状态。
    adv.SetTask(
        StreamBackupTaskInfo {
            Name: "demo".into(),
            ..Default::default()
        },
        vec![KeyRange {
            StartKey: b"a".to_vec(),
            EndKey: b"z".to_vec(),
        }],
    );
    assert!(adv.HasTask());
    adv.OnBecomeOwner();
    // 订阅拓扑后应有一条 store 订阅。
    let mut sub = NewSubscriber(cluster.clone(), Vec::new());
    sub.UpdateStoreTopology().unwrap();
    assert_eq!(sub.SubscriptionCount(), 1);
    // flush 间隔刷新后 resolve-lock 间隔应等于 FakeCluster.flush。
    adv.refreshLogBackupFlushInterval().unwrap();
    assert_eq!(adv.getResolveLockInterval(), Duration::from_secs(4));
    adv.OnTick().unwrap();
    // tick 成功后全局水位应升到 88。
    assert_eq!(cluster.GetGlobalCheckpointForTask("demo").unwrap(), 88);
    adv.OnStop();

    // Owner 管理路径与事件字符串契约。
    // 这些字符串被 owner 选举与日志标签使用，禁止随意改名。
    assert_eq!(OwnerManagerPrompt(), "log-backup");
    assert_eq!(OwnerManagerPath(), "/tidb/br-stream/owner");
    assert_eq!(EventType::EventAdd.to_string(), "Add");

    // ========== 分段 10：region 迭代与 collector ==========
    // region 迭代：单页扫完后 Done。
    // region iter Done/Next
    let mut iter = IterateRegion(cluster.as_ref(), b"a", b"z");
    assert!(!iter.Done());
    let page = iter.Next().unwrap();
    assert_eq!(page.len(), 1);
    assert!(iter.Done());

    // 直接走 collector：收集同一 region 后 Finish 应得检查点 88。
    // 与 advancer 路径交叉验证，避免只测到一边。
    // direct collector finish
    let mut coll = NewClusterCollector(cluster.clone());
    coll.CollectRegion(RegionWithLeader {
        Region: Region {
            Id: 1,
            StartKey: b"a".to_vec(),
            EndKey: b"z".to_vec(),
            ..Default::default()
        },
        Leader: Peer { Id: 11, StoreId: 7 },
    })
    .unwrap();
    let finished = coll.Finish().unwrap();
    assert!(finished.HasCheckpoint);
    assert_eq!(finished.Checkpoint, 88);
}

#[test]
fn task_metadata_methods_match_go_contract() {
    let kv = Arc::new(MemEtcd::new());
    let meta = NewMetaDataClient(kv.clone());
    let info = NewTaskInfo("task_methods")
        .FromTS(10)
        .WithTableFilter(&["*.*"])
        .ToStorage(StorageBackend {
            Uri: "noop://".into(),
        })
        .WithRanges(&[
            KeyRange {
                StartKey: b"a".to_vec(),
                EndKey: b"m".to_vec(),
            },
            KeyRange {
                StartKey: b"m".to_vec(),
                EndKey: b"z".to_vec(),
            },
        ]);
    meta.PutTask(&info).unwrap();
    let task = meta.GetTask("task_methods").unwrap();

    let mut initial = Vec::new();
    AdvancerExt { meta: meta.clone() }
        .BeginSnapshot(&mut initial)
        .unwrap();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].Ranges, info.Ranges);

    assert_eq!(task.Ranges().unwrap(), info.Ranges);
    assert!(!task.IsPaused().unwrap());
    task.Pause(vec![PauseWithMessage("maintenance".into())])
        .unwrap();
    assert!(task.IsPaused().unwrap());
    assert!(
        matches!(task.GetPauseV2().unwrap().unwrap().GetPayload().unwrap(), PausePayload::Text(v) if v == "maintenance")
    );
    task.Resume().unwrap();
    assert!(!task.IsPaused().unwrap());

    kv.Put(
        &format!("{}store/1", CheckPointsOf("task_methods")),
        &encodeUint64(30),
    )
    .unwrap();
    kv.Put(&StorageCheckpointOf("task_methods"), &encodeUint64(40))
        .unwrap();
    assert_eq!(task.NextBackupTSList().unwrap().len(), 1);
    assert_eq!(task.GetStorageCheckpoint().unwrap(), 40);
    assert_eq!(task.GetGlobalCheckPointTS().unwrap(), 40);
    task.UploadGlobalCheckpoint(50).unwrap();
    assert_eq!(task.GetGlobalCheckPointTS().unwrap(), 50);

    let err = StreamBackupError {
        ErrorCode: "E_STORE".into(),
        ErrorMessage: "broken".into(),
    };
    kv.Put(
        &format!("{}7", LastErrorPrefixOf("task_methods")),
        &err.Marshal().unwrap(),
    )
    .unwrap();
    assert_eq!(
        task.LastError().unwrap().get(&7).unwrap().ErrorCode,
        "E_STORE"
    );

    meta.DeleteTask("task_methods").unwrap();
    assert!(
        kv.Get(&GlobalCheckpointOf("task_methods"))
            .unwrap()
            .is_empty()
    );
    assert!(
        kv.GetPrefix(&StorageCheckpointOf("task_methods"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn pause_error_severity_and_rfc3339_time_match_go() {
    let mut pause = NewLocalPauseV2();
    PauseWithErrorSeverity(&mut pause);
    assert_eq!(pause.Severity, SeverityError);
    let now = pause.OperationTime.to_string();
    assert!(RFC3339Time::Parse(&now).is_ok());
    assert!(RFC3339Time::Parse("2026-08-13T12:34:56.123+08:00").is_ok());
    assert!(RFC3339Time::Parse("2026-99-99T99:99:99Z").is_err());
}

#[test]
fn export_test_config_updates_modify_the_current_config_like_go() {
    let cluster = Arc::new(FakeCluster::default());
    let adv = NewCheckpointAdvancerForTest(cluster);

    adv.UpdateConfigWith(|cfg| cfg.TickDuration = Duration::from_secs(7));
    adv.UpdateConfigWith(|cfg| cfg.TryAdvanceThreshold = Duration::from_secs(9));

    assert_eq!(adv.TESTResolveLockInterval(), Duration::from_secs(14));
    assert_eq!(adv.TESTDefaultStartPollThreshold(), Duration::from_secs(9));
}
