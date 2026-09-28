// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/aws/ebs_test.go`.
//!
//! External AWS EC2/CloudWatch calls are not made: `test_wait_snapshots_created`
//! consumes an in-memory mock `DescribeSnapshotsOutput` exactly as Go's
//! `testWaitSnapshotsCreated` does. Session methods use in-memory clients; one
//! regression constructs SDK error metadata without sending a request.
//!
//! 本文件对齐 Go `ebs_test.go`：不访问真实 EC2/CloudWatch，只验证进度解析、
//! volume 响应拆分与快照等待的状态机分支。
//! `StubEc2`/`StubCloudWatch` 仅为构造 `EC2Session` 所需，被测路径从不调用它们。
//! `test_wait_snapshots_created` 直接消费内存 mock，对应 Go 的测试专用等待函数。
//! pending 场景用永不完成的 channel recv 模拟超时，由测试侧 `recv_timeout` 打断。
//! 表驱动用例顺序与 Go 一致：全部完成、带消息失败、无消息失败、pending 超时。
//! 断言关注整数进度截断、已创建容量累计与错误文案，不把桩能力当作生产 AWS 行为。
//! 与 `parity_test.rs` 的分工：本文件紧贴 Go 单测形状；parity 覆盖更广公开 API 串联。
//! 进度解析必须截断而非四舍五入（44.99%→44），否则与 AWS 控制台展示不一致。
//! 超时用例不得 join 工作线程：pending 分支故意永不返回，依赖进程结束回收。

// 标准库：HashMap 承载 vol→snap；mpsc 模拟 Go channel 阻塞与超时。
use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use aws_sdk_ec2::error::{ErrorMetadata, SdkError};
use aws_sdk_ec2::operation::create_snapshots::CreateSnapshotsError;

// 被测类型与桩 trait 来自本 crate；SDK 类型仅用于构造无网络错误元数据。
use crate::{
    CloudWatchClient, CreateSnapshotsOutput, CreateVolumeOutput, DescribeVolumesOutput,
    EBSBasedBRMeta, EBSStore, EBSVolume, EC2Session, Ec2Client, FastSnapshotRestore,
    FsrUnsuccessfulItem, Instance, InstanceSpecification, Snapshot, SnapshotState, Tag,
    TiKVComponent, Volume, VolumeAttachment, VolumeState, aws_error_to_string,
};

/// 惰性 EC2 客户端：本文件单元测试不会真正走到 AWS API。
/// 各方法返回内存中的最小成功响应，不引入外部依赖。
/// 同时满足 `NewEC2SessionWithClients` 与 CreateSnapshots 编排测试。
#[derive(Default)]
struct StubEc2 {
    create_snapshots_output: Option<CreateSnapshotsOutput>,
}

impl Ec2Client for StubEc2 {
    // 按请求 ID 返回已挂载卷，支持 CreateSnapshots 编排测试。
    fn DescribeVolumes(&self, volume_ids: &[String]) -> crate::Result<Vec<Volume>> {
        Ok(volume_ids
            .iter()
            .map(|volume_id| Volume {
                VolumeId: volume_id.clone(),
                AvailabilityZone: Some("us-west-2a".into()),
                Attachments: vec![VolumeAttachment {
                    InstanceId: Some("i-test".into()),
                }],
                ..Default::default()
            })
            .collect())
    }
    // 实例查询桩：返回无额外块设备的目标实例。
    fn DescribeInstances(&self, _instance_ids: &[String]) -> crate::Result<Vec<Instance>> {
        Ok(vec![Instance::default()])
    }
    // 创建快照桩：返回用例配置输出，缺省为空。
    fn CreateSnapshots(
        &self,
        _instance_spec: &InstanceSpecification,
        _tags: &[Tag],
    ) -> crate::Result<CreateSnapshotsOutput> {
        Ok(self.create_snapshots_output.clone().unwrap_or_default())
    }
    // 描述快照桩：等待逻辑改走 mock output，不经此方法。
    fn DescribeSnapshots(&self, _snapshot_ids: &[String]) -> crate::Result<Vec<Snapshot>> {
        Ok(Vec::new())
    }
    // 删除快照桩：本套件无清理断言。
    fn DeleteSnapshot(&self, _snapshot_id: &str) -> crate::Result<()> {
        Ok(())
    }
    // 启用 FSR 桩：始终报告无失败项。
    fn EnableFastSnapshotRestores(
        &self,
        _availability_zones: &[String],
        _source_snapshot_ids: &[String],
    ) -> crate::Result<Vec<FsrUnsuccessfulItem>> {
        Ok(Vec::new())
    }
    // 禁用 FSR 桩：对称空成功。
    fn DisableFastSnapshotRestores(
        &self,
        _availability_zones: &[String],
        _source_snapshot_ids: &[String],
    ) -> crate::Result<Vec<FsrUnsuccessfulItem>> {
        Ok(Vec::new())
    }
    // 查询 FSR 状态桩。
    fn DescribeFastSnapshotRestores(
        &self,
        _states: &[String],
        _availability_zone: &str,
    ) -> crate::Result<Vec<FastSnapshotRestore>> {
        Ok(Vec::new())
    }
    // 从快照建卷桩：返回默认输出。
    fn CreateVolume(
        &self,
        _snapshot_id: &str,
        _availability_zone: &str,
        _volume_type: &str,
        _iops: Option<i32>,
        _throughput: Option<i32>,
        _encrypted: bool,
        _tags: &[Tag],
    ) -> crate::Result<CreateVolumeOutput> {
        Ok(CreateVolumeOutput::default())
    }
    // 删卷桩：无副作用断言。
    fn DeleteVolume(&self, _volume_id: &str) -> crate::Result<()> {
        Ok(())
    }
}

/// CloudWatch 桩：积分查询恒为 None，避免真实配额依赖。
#[derive(Default)]
/// CloudWatch 惰性桩；credit 查询恒 None。
struct StubCloudWatch;

impl CloudWatchClient for StubCloudWatch {
    // FSR 积分余额：单元测试不触发额度重试路径。
    fn GetFastSnapshotRestoreCreditsBalance(
        &self,
        _snapshot_id: &str,
        _availability_zone: &str,
    ) -> crate::Result<Option<f64>> {
        Ok(None)
    }
}

/// 构造带双桩客户端的会话；并发度 1 与 Go 单测轻量用法一致。
/// 构造带惰性客户端的会话；被测逻辑不依赖真实 API 返回。
fn new_test_session() -> EC2Session {
    EC2Session::NewEC2SessionWithClients(1, Arc::new(StubEc2::default()), Arc::new(StubCloudWatch))
}

#[test]
fn test_create_snapshots_preserves_empty_aws_ids_like_go() {
    let session = EC2Session::NewEC2SessionWithClients(
        1,
        Arc::new(StubEc2 {
            create_snapshots_output: Some(CreateSnapshotsOutput {
                Snapshots: vec![Snapshot::default()],
            }),
        }),
        Arc::new(StubCloudWatch),
    );
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

    let (snapshots, volume_azs, result) = session.CreateSnapshots(&meta);

    assert!(result.is_ok());
    assert_eq!(snapshots.get(""), Some(&String::new()));
    assert_eq!(volume_azs.get("vol-data-1"), Some(&"us-west-2a".into()));
}

#[test]
fn test_aws_service_error_preserves_code_and_message() {
    let service_error = CreateSnapshotsError::generic(
        ErrorMetadata::builder()
            .code("PendingSnapshotLimitExceeded")
            .message("pending snapshot quota reached")
            .build(),
    );
    let sdk_error = SdkError::service_error(service_error, ());

    let rendered = aws_error_to_string(&sdk_error);

    assert!(rendered.contains("PendingSnapshotLimitExceeded"));
    assert!(rendered.contains("pending snapshot quota reached"));
}

/// 镜像 Go `ec2.DescribeSnapshotsOutput` 夹具形状，供等待测试直接消费。
/// 生产路径用真实 SDK 类型；此处刻意本地化以免拉入 AWS SDK。
#[derive(Clone, Debug, Default)]
/// 测试专用 DescribeSnapshots 输出容器，对齐 Go mock 结构。
struct DescribeSnapshotsOutput {
    /// 模拟 DescribeSnapshots 返回的快照列表。
    Snapshots: Vec<Snapshot>,
}

// TestEC2SessionExtractSnapProgress 对应 Go 表驱动测试：解析 AWS progress 字符串并截断到整数百分比。
// 关键边界：None→0、小数截断、首尾空白、超过 100 钳制到 100。
#[test]
/// 表驱动：进度字符串解析与截断到整数百分比。
fn test_ec2_session_extract_snap_progress() {
    // 单用例：输入进度字符串与期望整数百分比。
    struct Case {
        str_value: Option<&'static str>,
        want: i64,
    }

    // 表顺序与 Go `TestEC2SessionExtractSnapProgress` 对齐，便于双侧 diff。
    let tests = [
        // 缺失进度：与尚未上报的快照一致。
        Case {
            str_value: None,
            want: 0,
        },
        // 小数部分丢弃，不四舍五入。
        Case {
            str_value: Some("12.12%"),
            want: 12,
        },
        // 接近进位仍截断，防止进度条“跳满”。
        Case {
            str_value: Some("44.99%"),
            want: 44,
        },
        // AWS 偶发带空白，解析前需 trim。
        Case {
            str_value: Some("  89.89%  "),
            want: 89,
        },
        // 整百无小数。
        Case {
            str_value: Some("100%"),
            want: 100,
        },
        // 异常超大百分比钳制到 100，避免进度溢出。
        Case {
            str_value: Some("111111%"),
            want: 100,
        },
        // fmt.Sscanf accepts trailing text after the literal percent.
        Case {
            str_value: Some("12%unexpected"),
            want: 12,
        },
    ];
    let e = new_test_session();

    // None 走无缓冲分支；Some 需可变 String，对齐 Go `*string` 可变语义。
    for tt in tests {
        let got = match tt.str_value {
            None => e.extract_snap_progress(None),
            Some(s) => {
                let mut owned = s.to_string();
                e.extract_snap_progress(Some(&mut owned))
            }
        };
        assert_eq!(tt.want, got);
    }

    let mut tab_padded = "\t12%\t".to_string();
    assert_eq!(12, e.extract_snap_progress(Some(&mut tab_padded)));
    assert_eq!("\t12%\t", tab_padded);
}

// create_volume 对应 Go 的 createVolume 测试辅助函数。
// Size=1、FastRestored=true，便于后续 FSR 相关断言复用同一夹具形状。
/// 构造最小 Volume 夹具，供 HandleDescribeVolumesResponse 用例。
fn create_volume(snapshot_id: &str, volume_id: &str, state: VolumeState) -> Volume {
    Volume {
        VolumeId: volume_id.to_string(),
        AvailabilityZone: Some("us-west-2".into()),
        State: state,
        Size: Some(1),
        SnapshotId: Some(snapshot_id.to_string()),
        FastRestored: Some(true),
        Attachments: Vec::new(),
    }
}

// TestHandleDescribeVolumesResponse 对应 Go 测试：统计已创建 volume 数量并找出未完成项。
// 夹具含 4 个 Available + 1 个 Creating；`fsr_required=false` 不校验 FastRestored。
#[test]
/// 验证已 Available 容量累计与未完成 volume 列表拆分。
fn test_handle_describe_volumes_response() {
    // 与 Go 相同的五卷夹具：唯一 Creating 应落入 unfinished。
    let current_volumes_states = DescribeVolumesOutput {
        Volumes: vec![
            create_volume("snap-0873674883", "vol-98768979", VolumeState::Available),
            create_volume("snap-0873674883", "vol-98768979", VolumeState::Creating),
            create_volume("snap-0873674883", "vol-98768979", VolumeState::Available),
            create_volume("snap-0873674883", "vol-98768979", VolumeState::Available),
            create_volume("snap-0873674883", "vol-98768979", VolumeState::Available),
        ],
    };

    let e = new_test_session();
    // 期望：已创建容量 4、未完成列表长度 1。
    let (created_volume_size, unfinished_volumes) = e
        .HandleDescribeVolumesResponse(&current_volumes_states, false)
        .expect("HandleDescribeVolumesResponse should succeed");
    assert_eq!(4_i64, created_volume_size);
    assert_eq!(1_usize, unfinished_volumes.len());
}

// 测试扩展：注入 mock DescribeSnapshots 序列，绕过 Ec2Client。
impl EC2Session {
    // test_wait_snapshots_created 对应 Go 的 testWaitSnapshotsCreated：不调用 EC2，直接消费 mock output。
    // pending 分支用永不完成的 recv 模拟 Go 的 `<-make(chan struct{})`，由调用方超时打断。
    // 超时由测试侧控制，避免死等真实 AWS。
    // Completed 累加 VolumeSize；Error 立即返回；其余状态记入 uncompleted。
    // 进度 map 只在“当前 > 历史”时更新，镜像生产 Wait 的单调进度上报。
    fn test_wait_snapshots_created(
        &self,
        snap_id_map: &HashMap<String, String>,
        mock_output: &DescribeSnapshotsOutput,
    ) -> Result<i64, String> {
        // pending_snapshots 与 Go 一样从 map 收集，本测试随后丢弃（真实 Wait 会轮询）。
        let mut pending_snapshots: Vec<String> = Vec::with_capacity(snap_id_map.len());
        for snap_id in snap_id_map.values() {
            pending_snapshots.push(snap_id.clone());
        }
        let _ = pending_snapshots;
        let mut total_volume_size = 0_i64;
        let mut snap_progress_map: HashMap<String, i64> = HashMap::with_capacity(snap_id_map.len());

        let resp = mock_output;
        let mut uncompleted_snapshots: Vec<String> = Vec::new();
        // 单次遍历完成：容量累计、错误短路、未完成收集与进度更新。
        for s in &resp.Snapshots {
            let snapshot_id = s.SnapshotId.clone();
            match &s.State {
                SnapshotState::Completed => {
                    if let Some(volume_size) = s.VolumeSize {
                        total_volume_size += i64::from(volume_size);
                    }
                }
                SnapshotState::Error => {
                    return Err(format!("snapshot {snapshot_id} failed"));
                }
                _ => {
                    if !snapshot_id.is_empty() {
                        uncompleted_snapshots.push(snapshot_id.clone());
                    }
                }
            }

            // Progress 可能为 None；extract 后与历史比较再写回。
            let mut progress = s.Progress.clone();
            let curr_snap_progress = self.extract_snap_progress(progress.as_mut());
            let previous = snap_progress_map.get(&snapshot_id).copied().unwrap_or(0);
            if curr_snap_progress > previous {
                snap_progress_map.insert(snapshot_id, curr_snap_progress);
            }
        }

        if !uncompleted_snapshots.is_empty() {
            // Go: `<-make(chan struct{})` — block forever until the test timeout wins.
            // 保留 tx 存活，避免 recv 因断开立即返回而误判“已完成”。
            let (_keep_alive, rx) = mpsc::channel::<()>();
            let _ = rx.recv();
        }

        Ok(total_volume_size)
    }
}

// TestWaitSnapshotsCreated 对应 Go 表驱动测试：覆盖 completed、error、无状态消息 error 和 pending 超时。
// 超时用例：工作线程卡在 pending 分支，主线程 50ms 内未收到完成信号即判定契约成立。
// Go 用 context.WithTimeout(..., 6)（6ns）；Rust 用毫秒级短超时，语义仍是“超时先于返回”。
#[test]
/// 对齐 Go testWaitSnapshotsCreated：完成/失败/pending 超时四场景。
fn test_wait_snapshots_created() {
    // vol→snap 映射仅提供 ID 集合；实际状态完全由 mock_output 决定。
    let snap_id_map = HashMap::from([
        ("vol-1".to_string(), "snap-1".to_string()),
        ("vol-2".to_string(), "snap-2".to_string()),
    ]);

    // expect_timeout 与 expect_err 互斥：超时路径不检查 size。
    struct Case {
        desc: &'static str,
        snapshots_output: DescribeSnapshotsOutput,
        expected_size: i64,
        expect_err: bool,
        expect_timeout: bool,
    }

    // 四场景顺序对齐 Go：全完成→失败→无消息失败→pending。
    let cases = vec![
        Case {
            desc: "snapshots are all completed",
            snapshots_output: DescribeSnapshotsOutput {
                Snapshots: vec![
                    Snapshot {
                        SnapshotId: "snap-1".into(),
                        VolumeSize: Some(1),
                        State: SnapshotState::Completed,
                        ..Default::default()
                    },
                    Snapshot {
                        SnapshotId: "snap-2".into(),
                        VolumeSize: Some(2),
                        State: SnapshotState::Completed,
                        ..Default::default()
                    },
                ],
            },
            // 1+2=3，验证跨快照累加而非取最大值。
            expected_size: 3,
            expect_err: false,
            expect_timeout: false,
        },
        Case {
            // 部分完成仍因另一快照 Error 整体失败。
            desc: "snapshot failed",
            snapshots_output: DescribeSnapshotsOutput {
                Snapshots: vec![
                    Snapshot {
                        SnapshotId: "snap-1".into(),
                        VolumeSize: Some(1),
                        State: SnapshotState::Completed,
                        ..Default::default()
                    },
                    Snapshot {
                        SnapshotId: "snap-2".into(),
                        State: SnapshotState::Error,
                        StateMessage: Some("snapshot failed".into()),
                        ..Default::default()
                    },
                ],
            },
            expected_size: 0,
            expect_err: true,
            expect_timeout: false,
        },
        Case {
            desc: "snapshot failed w/out state message",
            // 无 StateMessage 仍应失败：错误文案由 snapshot_id 拼出，不依赖消息字段。
            snapshots_output: DescribeSnapshotsOutput {
                Snapshots: vec![
                    Snapshot {
                        SnapshotId: "snap-1".into(),
                        VolumeSize: Some(1),
                        State: SnapshotState::Completed,
                        ..Default::default()
                    },
                    Snapshot {
                        SnapshotId: "snap-2".into(),
                        State: SnapshotState::Error,
                        StateMessage: None,
                        ..Default::default()
                    },
                ],
            },
            expected_size: 0,
            expect_err: true,
            expect_timeout: false,
        },
        Case {
            desc: "snapshots pending",
            snapshots_output: DescribeSnapshotsOutput {
                Snapshots: vec![
                    Snapshot {
                        SnapshotId: "snap-1".into(),
                        VolumeSize: Some(1),
                        State: SnapshotState::Completed,
                        ..Default::default()
                    },
                    Snapshot {
                        SnapshotId: "snap-2".into(),
                        State: SnapshotState::Pending,
                        ..Default::default()
                    },
                ],
            },
            // pending 不得当成成功：必须卡住直到测试超时。
            expected_size: 0,
            expect_err: false,
            expect_timeout: true,
        },
    ];

    for c in cases {
        // 每用例独立会话，避免共享状态泄漏。
        let e = new_test_session();

        if c.expect_timeout {
            // Go: context.WithTimeout(..., 6) — untyped 6 is 6ns; use a short bound that still
            // proves the pending branch does not return before the deadline.
            // 工作线程阻塞；超时即成功，提前返回则 panic。
            let timeout = Duration::from_millis(50);
            let snap_id_map = snap_id_map.clone();
            let snapshots_output = c.snapshots_output.clone();
            let (done_tx, done_rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = e.test_wait_snapshots_created(&snap_id_map, &snapshots_output);
                let _ = done_tx.send(());
            });

            match done_rx.recv_timeout(timeout) {
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // expected: pending branch blocks until test timeout
                }
                Ok(()) => {
                    panic!(
                        "{}: testWaitSnapshotsCreated should not return before timeout",
                        c.desc
                    )
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("{}: worker disconnected before timeout", c.desc)
                }
            }
            continue;
        }

        // 非超时路径：校验 Result 与期望容量（失败时 unwrap_or(0) 对齐 Go 零值习惯）。
        let size = e.test_wait_snapshots_created(&snap_id_map, &c.snapshots_output);
        if c.expect_err {
            assert!(size.is_err(), "{}: expected error", c.desc);
            assert_eq!(c.expected_size, size.unwrap_or(0), "{}", c.desc);
        } else {
            assert!(size.is_ok(), "{}: {:?}", c.desc, size);
            assert_eq!(c.expected_size, size.unwrap(), "{}", c.desc);
        }
    }
}
