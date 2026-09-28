// Copyright 2026 AsterSQL.

//! AWS 包 Go/Rust 公开契约对齐测试（无独立 Go 同名文件，场景聚合自 `ebs_test.go` 与实现）。
//!
//! 本文件用内存 `MemEc2`/`MemCloudWatch` 注入 `EC2Session`，覆盖进度解析、volume 拆分、
//! 目标快照筛选、FSR、创建/删除快照与卷、以及 WaitSnapshotsCreated 成功/失败路径。
//! 计时器全部压到 1ms，避免单测被真实轮询间隔拖慢，同时仍走重试分支。
//! `MemProgress` 记录 IncBy 累计，用于断言 Wait 路径的增量进度上报。
// 注入限流错误，下一调用成功。
//! `CreateSnapshots` 的 PendingSnapshotLimitExceeded 错误队列验证重试后成功。
//! 不把内存桩描述为生产 AWS；断言聚焦可观察 map/错误文案/调用次数契约。
//! 与 `ebs_test.rs` 分工：后者对齐 Go 表驱动细节，本文件覆盖更广的公开 API 串联。
//! 场景顺序大致：构造约束 → 进度/拆分 → fetch_target_snapshots → FSR →
//! Create/Wait/Delete Snapshots/Volumes → 限流重试。
//! 每个子断言只锁定可观察输出（map、错误串、调用次数），不依赖真实 AWS 时序。
//! `DescribeInstances` 固定返回 root/extra/data 三块设备，专门考验“只快照 meta 声明卷”。
//! `fetch_target_snapshots` 过滤 `storage.data-dir`，wal 卷不得进入恢复目标集合。
//! `NewEC2Session` 使用内置 WebPKI roots，隔离/空 native trust store 也必须可构造。
//! DisableDataFSR 空 map 为幂等 no-op；非空 map 走禁用 API 且不应残留错误。
//! Wait 失败路径错误串须含 snapshot id，便于与 Go errors.Errorf 文案对照。
//! CreateVolumes 映射键为备份元数据中的原卷 ID，值为新建 `vol-from-*`。
//! EnableDataFSR 对空 meta 的错误文案须含 `empty backup meta`，与 Go 侧一致。
//! HandleDescribeVolumesResponse 在 fsr_required 时拒绝 FastRestored=false。
//! sample_meta 同时放 data-dir 与 wal，是过滤逻辑的最小对照夹具。
//! MemEc2.CreateSnapshots 成功态固定 Pending，逼近真实“刚创建未完成”时序。
//! 长测试分段顺序：解析→拆卷→筛选→构造失败→FSR→创建删除→重试→建卷→禁用→Wait。
//! 任意一段断言失败即可对照 Go 同语义段落，无需重跑全文件定位。
//! deleted_* 向量只追加不清理，单用例内长度断言足够；跨用例靠新建 MemEc2。
//! CloudWatch balances 默认为空，本文件未覆盖积分重试耗尽分支（留给实现单测）。
//! Tag 传播在 CreateVolumes 路径依赖 DescribeSnapshots 返回的 Tags 字段。

// 标准库：HashMap/Mutex 支撑可复现的内存云资源表。
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// 公开 API 与错误类型均从 crate 根导入，保持与库使用者相同入口。
use crate::{
    CloudWatchClient, CreateSnapshotsOutput, DescribeVolumesOutput, EBSBasedBRMeta, EBSStore,
    EBSVolume, EC2Session, Ec2ApiError, Ec2Client, FastSnapshotRestore, FsrUnsuccessfulItem,
    NewEC2Session, Snapshot, SnapshotState, Tag, TiKVComponent, Volume, VolumeAttachment,
    VolumeState, fetch_target_snapshots,
};

/// 线程安全进度累加器，对齐 Go 测试里可观测的 progress sink。
struct MemProgress {
    /// 当前累计值；Wait 路径通过 IncBy 写入。
    current: Mutex<i64>,
}

impl MemProgress {
    /// 从 0 开始，避免跨用例污染。
    fn new() -> Self {
        Self {
            current: Mutex::new(0),
        }
    }

    /// 读取累计进度，供断言。
    fn current(&self) -> i64 {
        *self.current.lock().unwrap()
    }
}

impl crate::Progress for MemProgress {
    /// 与 Go Progress.IncBy 语义一致：只增不减。
    fn IncBy(&self, count: i64) {
        *self.current.lock().unwrap() += count;
    }
}

/// 内存 EC2：按 ID 索引卷/快照，并记录删除与 CreateSnapshots 调用次数。
/// `create_snapshots_errors` 按调用序号弹出，用于模拟限流后成功。
#[derive(Default)]
struct MemEc2 {
    /// 已登记卷；Describe/Create 路径共享。
    // VolumeId → Volume。
    volumes: Mutex<HashMap<String, Volume>>,
    /// 已登记快照；Wait/CreateVolumes 读取。
    // SnapshotId → Snapshot。
    snapshots: Mutex<HashMap<String, Snapshot>>,
    /// CreateSnapshots 调用计数（含失败次）。
    // CreateSnapshots 调用计数。
    create_snapshots_calls: Mutex<usize>,
    /// 按调用序消费的错误队列。
    // 按次弹出的错误队列（限流模拟）。
    create_snapshots_errors: Mutex<Vec<Ec2ApiError>>,
    /// DeleteSnapshot 观测日志。
    // 已删除快照 id，供断言。
    deleted_snapshots: Mutex<Vec<String>>,
    /// DeleteVolume 观测日志。
    // 已删除卷 id。
    deleted_volumes: Mutex<Vec<String>>,
    /// DescribeFastSnapshotRestores 返回值。
    // DescribeFastSnapshotRestores 返回集。
    fsr_restores: Mutex<Vec<FastSnapshotRestore>>,
    /// EnableFastSnapshotRestores 预设失败项。
    // Enable FSR 失败项注入。
    enable_fsr_unsuccessful: Mutex<Vec<FsrUnsuccessfulItem>>,
}

impl MemEc2 {
    /// 以 VolumeId 为键写入，供后续 DescribeVolumes 命中。
    fn insert_volume(&self, volume: Volume) {
        self.volumes
            .lock()
            .unwrap()
            .insert(volume.VolumeId.clone(), volume);
    }

    /// 以 SnapshotId 为键写入，供 Wait/CreateVolumes 查询。
    fn insert_snapshot(&self, snapshot: Snapshot) {
        self.snapshots
            .lock()
            .unwrap()
            .insert(snapshot.SnapshotId.clone(), snapshot);
    }

    /// 追加下一次 CreateSnapshots 应返回的 API 错误。
    fn push_create_snapshots_error(&self, err: Ec2ApiError) {
        self.create_snapshots_errors.lock().unwrap().push(err);
    }
}

impl Ec2Client for MemEc2 {
    /// 仅返回请求 ID 集合中已插入的卷。
    // 按 id 取内存卷；缺失则跳过。
    fn DescribeVolumes(&self, volume_ids: &[String]) -> crate::Result<Vec<Volume>> {
        let map = self.volumes.lock().unwrap();
        Ok(volume_ids
            .iter()
            .filter_map(|id| map.get(id).cloned())
            .collect())
    }

    /// 固定块设备映射：含 root、额外 data 与目标 `vol-data-1`，验证排除非目标卷。
    fn DescribeInstances(&self, _instance_ids: &[String]) -> crate::Result<Vec<crate::Instance>> {
        Ok(vec![crate::Instance {
            RootDeviceName: Some("/dev/xvda".into()),
            BlockDeviceMappings: vec![
                crate::BlockDeviceMapping {
                    DeviceName: Some("/dev/xvda".into()),
                    Ebs: Some(crate::EbsBlockDevice {
                        VolumeId: Some("vol-root".into()),
                    }),
                },
                crate::BlockDeviceMapping {
                    DeviceName: Some("/dev/xvdf".into()),
                    Ebs: Some(crate::EbsBlockDevice {
                        VolumeId: Some("vol-data-extra".into()),
                    }),
                },
                crate::BlockDeviceMapping {
                    DeviceName: Some("/dev/xvdg".into()),
                    Ebs: Some(crate::EbsBlockDevice {
                        VolumeId: Some("vol-data-1".into()),
                    }),
                },
            ],
        }])
    }

    /// 前 N 次调用按错误队列失败，之后返回固定 snap-new→vol-data-1。
    // 弹出错误或登记调用并生成快照。
    fn CreateSnapshots(
        &self,
        instance_spec: &crate::InstanceSpecification,
        _tags: &[Tag],
    ) -> crate::Result<CreateSnapshotsOutput> {
        let mut calls = self.create_snapshots_calls.lock().unwrap();
        *calls += 1;
        let pending_errors = self.create_snapshots_errors.lock().unwrap();
        if *calls <= pending_errors.len() {
            let err = pending_errors[*calls - 1].clone();
            return Err(err.to_string());
        }
        drop(calls);
        drop(pending_errors);

        let _ = instance_spec;
        Ok(CreateSnapshotsOutput {
            Snapshots: vec![Snapshot {
                SnapshotId: "snap-new".into(),
                VolumeId: "vol-data-1".into(),
                State: SnapshotState::Pending,
                ..Default::default()
            }],
        })
    }

    /// 按 ID 过滤已插入快照。
    // 按 id 查询内存快照。
    fn DescribeSnapshots(&self, snapshot_ids: &[String]) -> crate::Result<Vec<Snapshot>> {
        let map = self.snapshots.lock().unwrap();
        Ok(snapshot_ids
            .iter()
            .filter_map(|id| map.get(id).cloned())
            .collect())
    }

    /// 记录删除顺序，供 best-effort 清理断言。
    // 记录删除并移除。
    fn DeleteSnapshot(&self, snapshot_id: &str) -> crate::Result<()> {
        self.deleted_snapshots
            .lock()
            .unwrap()
            .push(snapshot_id.to_string());
        Ok(())
    }

    /// 若预设 unsuccessful 非空则原样返回，否则视为全部成功。
    // 返回注入的 unsuccessful 列表。
    fn EnableFastSnapshotRestores(
        &self,
        _availability_zones: &[String],
        source_snapshot_ids: &[String],
    ) -> crate::Result<Vec<FsrUnsuccessfulItem>> {
        let unsuccessful = self.enable_fsr_unsuccessful.lock().unwrap().clone();
        if !unsuccessful.is_empty() {
            return Ok(unsuccessful);
        }
        let _ = source_snapshot_ids;
        Ok(Vec::new())
    }

    /// 禁用 FSR：本套件只验证调用不报错。
    // 空成功，便于 DisableDataFSR 路径。
    fn DisableFastSnapshotRestores(
        &self,
        _availability_zones: &[String],
        _source_snapshot_ids: &[String],
    ) -> crate::Result<Vec<FsrUnsuccessfulItem>> {
        Ok(Vec::new())
    }

    /// 返回预设 FSR 列表（可为默认空）。
    // 返回预置 FSR 状态集。
    fn DescribeFastSnapshotRestores(
        &self,
        _states: &[String],
        _availability_zone: &str,
    ) -> crate::Result<Vec<FastSnapshotRestore>> {
        Ok(self.fsr_restores.lock().unwrap().clone())
    }

    /// 从快照建卷：生成 `vol-from-{snapshot_id}` 并写入 volumes 表。
    // 分配新 VolumeId 并写入 map。
    fn CreateVolume(
        &self,
        snapshot_id: &str,
        availability_zone: &str,
        _volume_type: &str,
        _iops: Option<i32>,
        _throughput: Option<i32>,
        _encrypted: bool,
        tags: &[Tag],
    ) -> crate::Result<crate::CreateVolumeOutput> {
        let volume_id = format!("vol-from-{snapshot_id}");
        self.insert_volume(Volume {
            VolumeId: volume_id.clone(),
            AvailabilityZone: Some(availability_zone.to_string()),
            State: VolumeState::Available,
            Size: Some(10),
            SnapshotId: Some(snapshot_id.to_string()),
            FastRestored: Some(true),
            ..Default::default()
        });
        let _ = tags;
        Ok(crate::CreateVolumeOutput {
            VolumeId: Some(volume_id),
        })
    }

    /// 记录删卷 ID，不真正移除 map（足以断言清理被调用）。
    // 记录删除。
    fn DeleteVolume(&self, volume_id: &str) -> crate::Result<()> {
        self.deleted_volumes
            .lock()
            .unwrap()
            .push(volume_id.to_string());
        Ok(())
    }
}

/// CloudWatch 内存实现：按 snapshot_id 查 FSR 积分。
#[derive(Default)]
/// 可配置 credit 返回值的 CloudWatch 内存桩。
struct MemCloudWatch {
    balances: Mutex<HashMap<String, f64>>,
}

impl CloudWatchClient for MemCloudWatch {
    /// 未插入则返回 None，触发实现侧“无额度/未知”分支。
    // 返回配置的 credit。
    fn GetFastSnapshotRestoreCreditsBalance(
        &self,
        snapshot_id: &str,
        _availability_zone: &str,
    ) -> crate::Result<Option<f64>> {
        Ok(self.balances.lock().unwrap().get(snapshot_id).copied())
    }
}

/// 标准备份元数据：含 data-dir 与 wal 卷，验证只挑选 data-dir 快照。
/// StoreID=1 足够覆盖单 store 路径；多 store 行为不在本契约测试范围。
/// 构造含多 store/volume 的样例元数据，覆盖 AZ 筛选与并发路径。
fn sample_meta() -> EBSBasedBRMeta {
    EBSBasedBRMeta {
        TiKVComponent: Some(TiKVComponent {
            Stores: vec![EBSStore {
                StoreID: 1,
                Volumes: vec![
                    // data-dir：恢复目标，必须进入 by_az。
                    EBSVolume {
                        ID: "vol-data-1".into(),
                        Type: "storage.data-dir".into(),
                        SnapshotID: "snap-1".into(),
                        VolumeAZ: "us-west-2a".into(),
                        ..Default::default()
                    },
                    // wal：同 AZ 但类型不同，必须被过滤掉。
                    EBSVolume {
                        ID: "vol-wal".into(),
                        Type: "storage.wal".into(),
                        SnapshotID: "snap-wal".into(),
                        VolumeAZ: "us-west-2a".into(),
                        ..Default::default()
                    },
                ],
            }],
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// 注入客户端并压缩所有轮询间隔，保证重试路径可测且快速。
/// 并发度 2：与生产默认不同，但足以覆盖多 worker 拼装而不引入竞态噪声。
/// 注入内存客户端，并将全部 timers 压到 1ms 以加速轮询。
fn new_session(ec2: Arc<MemEc2>, cloudwatch: Arc<MemCloudWatch>) -> EC2Session {
    EC2Session::NewEC2SessionWithClients(2, ec2, cloudwatch).with_timers(crate::EC2SessionTimers {
        // 下列间隔仅影响睡眠，不改变重试次数语义。
        pending_snapshot_retry: Duration::from_millis(1),
        snapshot_poll: Duration::from_millis(1),
        volume_poll: Duration::from_millis(1),
        fsr_credit_retry: Duration::from_millis(1),
        fsr_state_poll: Duration::from_millis(1),
    })
}

/// 与 ebs_test 同源的 volume 夹具构造。
/// FastRestored 默认 true，单独用例再覆盖 false 以触发 FSR 校验。
/// parity 夹具 Volume；与 ebs_test 同形但服务串联用例。
fn create_volume(snapshot_id: &str, volume_id: &str, state: VolumeState) -> Volume {
    Volume {
        VolumeId: volume_id.to_string(),
        AvailabilityZone: Some("us-west-2".into()),
        State: state,
        Size: Some(1),
        SnapshotId: Some(snapshot_id.to_string()),
        FastRestored: Some(true),
        ..Default::default()
    }
}

/// 单一长测试串联公开契约：正常路径、边界筛选、错误与资源清理。
/// 分段注释对应 Go 表驱动/集成场景，失败时按段落定位断言意图。
#[test]
/// 公开 API 串联契约：创建/等待/删除快照与卷、FSR、进度与错误文案。
fn go_rust_public_contract_matches() {
    // Normal: progress parsing mirrors Go table test.
    // 含非法 "bad"→0，覆盖 ebs_test 未列的解析失败回落。
    let session = new_session(
        Arc::new(MemEc2::default()),
        Arc::new(MemCloudWatch::default()),
    );
    assert_eq!(session.extract_snap_progress(None), 0);
    assert_eq!(
        session.extract_snap_progress(Some(&mut "12.12%".into())),
        12
    );
    assert_eq!(
        session.extract_snap_progress(Some(&mut "44.99%".into())),
        44
    );
    assert_eq!(
        session.extract_snap_progress(Some(&mut "  89.89%  ".into())),
        89
    );
    assert_eq!(session.extract_snap_progress(Some(&mut "100%".into())), 100);
    assert_eq!(
        session.extract_snap_progress(Some(&mut "111111%".into())),
        100
    );
    assert_eq!(session.extract_snap_progress(Some(&mut "bad".into())), 0);

    // Normal: HandleDescribeVolumesResponse size / unfinished split.
    // 4 Available + 1 Creating；fsr_required=false。
    let describe = DescribeVolumesOutput {
        Volumes: vec![
            create_volume("snap-0873674883", "vol-a", VolumeState::Available),
            create_volume("snap-0873674883", "vol-b", VolumeState::Creating),
            create_volume("snap-0873674883", "vol-c", VolumeState::Available),
            create_volume("snap-0873674883", "vol-d", VolumeState::Available),
            create_volume("snap-0873674883", "vol-e", VolumeState::Available),
        ],
    };
    let (created_volume_size, unfinished_volumes) = session
        .HandleDescribeVolumesResponse(&describe, false)
        .unwrap();
    assert_eq!(created_volume_size, 4);
    assert_eq!(unfinished_volumes.len(), 1);

    // Boundary: fetchTargetSnapshots only keeps data-dir volumes and respects AZ override.
    // 空 AZ 用卷自带 AZ；非空则强制归入指定 AZ；空 meta 得空 map。
    let meta = sample_meta();
    // 验证按目标 AZ 归类快照。
    let by_az = fetch_target_snapshots(&meta, "");
    assert_eq!(by_az.get("us-west-2a").map(Vec::len), Some(1));
    assert_eq!(by_az["us-west-2a"][0], "snap-1");
    // 验证按目标 AZ 归类快照。
    let forced_az = fetch_target_snapshots(&meta, "us-east-1a");
    assert_eq!(forced_az.get("us-east-1a").map(Vec::len), Some(1));
    // 验证按目标 AZ 归类快照。
    let empty = fetch_target_snapshots(&EBSBasedBRMeta::default(), "x");
    assert!(empty.is_empty());

    // Normal: loading AWS configuration and constructing live clients does not require an API call.
    // 当前测试环境没有可解析 native roots；构造器必须改用安全的内置 WebPKI roots，
    // 同时仍加载 region/standard retry(9) 并创建 EC2/CloudWatch 客户端。
    assert!(NewEC2Session(1, "us-west-2").is_ok());

    // Error: EnableDataFSR rejects empty backup meta.
    // 默认空内存 EC2。
    let ec2 = Arc::new(MemEc2::default());
    let cw = Arc::new(MemCloudWatch::default());
    let session = new_session(Arc::clone(&ec2), Arc::clone(&cw));
    // FSR 启用路径：批处理与 credit 等待。
    let (_, err) = session.EnableDataFSR(&EBSBasedBRMeta::default(), "us-west-2a");
    assert!(err.is_err());
    assert!(err.unwrap_err().to_string().contains("empty backup meta"));

    // Error: FSR required but volume not fast restored.
    // fsr_required=true 且 FastRestored=false 必须报 not fsr enabled。
    let fsr_err = session
        .HandleDescribeVolumesResponse(
            &DescribeVolumesOutput {
                Volumes: vec![Volume {
                    VolumeId: "vol-x".into(),
                    State: VolumeState::Available,
                    SnapshotId: Some("snap-x".into()),
                    FastRestored: Some(false),
                    ..Default::default()
                }],
            },
            true,
        )
        .unwrap_err();
    assert!(fsr_err.to_string().contains("not fsr enabled"));

    // Normal + resource cleanup: CreateSnapshots excludes non-target volumes and DeleteSnapshots best-effort.
    // meta 只声明 vol-data-1；实例上另有 root/extra 卷，映射中应仅出现目标卷。
    let ec2 = Arc::new(MemEc2::default());
    ec2.insert_volume(Volume {
        VolumeId: "vol-data-1".into(),
        AvailabilityZone: Some("us-west-2a".into()),
        Attachments: vec![VolumeAttachment {
            InstanceId: Some("i-123".into()),
        }],
        ..Default::default()
    });
    let session = new_session(Arc::clone(&ec2), Arc::new(MemCloudWatch::default()));
    let meta = EBSBasedBRMeta {
        TiKVComponent: Some(TiKVComponent {
            Stores: vec![EBSStore {
                StoreID: 1,
                Volumes: vec![EBSVolume {
                    ID: "vol-data-1".into(),
                    ..Default::default()
                }],
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    // 含限流重试的创建快照。
    let (snap_map, vol_azs, result) = session.CreateSnapshots(&meta);
    assert!(result.is_ok());
    // snap-new 来自 MemEc2 成功分支固定输出。
    assert_eq!(snap_map.get("vol-data-1"), Some(&"snap-new".into()));
    // AZ 取自已插入 volume，而非 DescribeInstances。
    assert_eq!(vol_azs.get("vol-data-1"), Some(&"us-west-2a".into()));

    // best-effort：即使后续失败也应至少尝试删除已创建快照。
    // 删除快照并核对 deleted 列表。
    session.DeleteSnapshots(snap_map);
    assert_eq!(ec2.deleted_snapshots.lock().unwrap().len(), 1);

    // 注入限流错误，下一调用成功。
    // Error: createSnapshotsWithRetry sleeps through PendingSnapshotLimitExceeded then succeeds.
    // 第一次失败、第二次成功，调用计数必须为 2。
    let ec2 = Arc::new(MemEc2::default());
    ec2.push_create_snapshots_error(Ec2ApiError {
        // 注入限流错误，下一调用成功。
        code: "PendingSnapshotLimitExceeded".into(),
        message: "limit".into(),
    });
    ec2.insert_volume(Volume {
        VolumeId: "vol-data-1".into(),
        AvailabilityZone: Some("us-west-2a".into()),
        Attachments: vec![VolumeAttachment {
            InstanceId: Some("i-123".into()),
        }],
        ..Default::default()
    });
    let session = new_session(Arc::clone(&ec2), Arc::new(MemCloudWatch::default()));
    // 含限流重试的创建快照。
    let (_, _, result) = session.CreateSnapshots(&meta);
    assert!(result.is_ok());
    assert_eq!(*ec2.create_snapshots_calls.lock().unwrap(), 2);

    // Normal: CreateVolumes + DeleteVolumes cleanup.
    // 快照带 tag 时 CreateVolumes 应传播；映射键为原卷 ID。
    let ec2 = Arc::new(MemEc2::default());
    ec2.insert_snapshot(Snapshot {
        SnapshotId: "snap-1".into(),
        Tags: vec![Tag {
            Key: "env".into(),
            Value: "prod".into(),
        }],
        ..Default::default()
    });
    let session = new_session(Arc::clone(&ec2), Arc::new(MemCloudWatch::default()));
    let create_volumes_meta = EBSBasedBRMeta {
        TiKVComponent: Some(TiKVComponent {
            Stores: vec![EBSStore {
                StoreID: 1,
                Volumes: vec![EBSVolume {
                    ID: "vol-data-1".into(),
                    SnapshotID: "snap-1".into(),
                    VolumeAZ: "us-west-2a".into(),
                    ..Default::default()
                }],
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    // gp3/3000/125/encrypted 参数透传给 CreateVolume；本桩忽略具体数值但仍需调用成功。
    let (volume_map, result) =
    // 从快照建卷并检查 id 映射。
        session.CreateVolumes(&create_volumes_meta, "gp3", 3000, 125, true, "");
    assert!(result.is_ok());
    assert_eq!(
        volume_map.get("vol-data-1"),
        Some(&"vol-from-snap-1".into())
    );
    // 清理侧按 map 值删除，长度应为 1。
    // 尽力删除新卷。
    session.DeleteVolumes(volume_map);
    assert_eq!(ec2.deleted_volumes.lock().unwrap().len(), 1);

    // Normal: DisableDataFSR empty map is no-op; non-empty succeeds with fake client.
    let session = new_session(
        Arc::new(MemEc2::default()),
        Arc::new(MemCloudWatch::default()),
    );
    // FSR 禁用对称路径。
    assert!(session.DisableDataFSR(HashMap::new()).is_ok());
    let mut disable_map = HashMap::new();
    disable_map.insert("us-west-2a".into(), vec!["snap-1".into()]);
    // FSR 禁用对称路径。
    assert!(session.DisableDataFSR(disable_map).is_ok());

    // Normal: WaitSnapshotsCreated reports incremental progress for completed snapshots.
    // VolumeSize=1、Progress=100% → 返回容量 1，进度累计 100。
    let ec2 = Arc::new(MemEc2::default());
    ec2.insert_snapshot(Snapshot {
        SnapshotId: "snap-1".into(),
        State: SnapshotState::Completed,
        VolumeSize: Some(1),
        Progress: Some("100%".into()),
        ..Default::default()
    });
    let session = new_session(Arc::clone(&ec2), Arc::new(MemCloudWatch::default()));
    let progress = MemProgress::new();
    let mut snap_map = HashMap::new();
    snap_map.insert("vol-1".into(), "snap-1".into());
    // 等待完成并校验进度累计。
    let size = session.WaitSnapshotsCreated(snap_map, &progress).unwrap();
    assert_eq!(size, 1);
    // 进度只增：与 IncBy 契约一致。
    assert_eq!(progress.current(), 100);

    // Error: WaitSnapshotsCreated fails on errored snapshot.
    // 错误文案须包含 snap-bad，便于运维定位。
    ec2.insert_snapshot(Snapshot {
        SnapshotId: "snap-bad".into(),
        State: SnapshotState::Error,
        StateMessage: Some("snapshot failed".into()),
        ..Default::default()
    });
    let progress = MemProgress::new();
    let mut snap_map = HashMap::new();
    snap_map.insert("vol-2".into(), "snap-bad".into());
    let err = session
        .WaitSnapshotsCreated(snap_map, &progress)
        .unwrap_err();
    assert!(err.to_string().contains("snap-bad"));
}
