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

//! AWS EBS snapshot / volume operations ported from `br/pkg/aws/ebs.go`.
//!
//! EC2 and CloudWatch are accessed through injectable traits so unit tests never
//! need live AWS credentials. Concurrency mirrors Go `errgroup` + `WorkerPool`
//! via `std::thread` and a first-error join.
//!
//! BR 云盘备份/恢复的 AWS 适配层：围绕 `EC2Session` 编排快照创建、等待、删除，
//! 以及卷创建/等待/删除与 Fast Snapshot Restore（FSR）启停。
//! `NewEC2Session` 接入官方 AWS EC2/CloudWatch SDK；测试可通过 `Ec2Client`/
//! `CloudWatchClient` trait 注入内存客户端。
//! 元数据形状对齐 `br/pkg/config` 的 EBSBasedBRMeta；并发模型对齐 Go errgroup+WorkerPool。
//! 错误以 String/`Ec2ApiError` 传递，首错汇聚后提前返回。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use aws_config::{BehaviorVersion, retry::RetryConfig};
use aws_sdk_cloudwatch::types::{Dimension, Statistic};
use aws_sdk_ec2::error::ProvideErrorMetadata;
use aws_sdk_ec2::types as aws_ec2_types;
use aws_types::region::Region;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

/// 本模块统一 Result；错误消息为 String，便于与注入客户端对齐。
pub type Result<T> = std::result::Result<T, String>;

// Pending 快照超限时的重试间隔默认值（与 Go 一致）。
const POLLING_PENDING_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(30);
// AWS Pending 快照配额错误码；create_snapshots_with_retry 据此退避。
const ERR_CODE_TOO_MANY_PENDING_SNAPSHOTS: &str = "PendingSnapshotLimitExceeded";
/// 单次 Enable/Disable FSR API 的快照批大小上限（AWS 约束）。
pub const FsrApiSnapshotsThreshold: usize = 10;

/// Local meta types matching `br/pkg/config` shapes used by EBS flows.
/// 下列结构由备份元数据 JSON 反序列化，驱动按 store/volume 的云盘操作。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
/// 单个 EBS 卷在 BR 元数据中的视图（含快照/恢复卷 id 与 AZ）。
pub struct EBSVolume {
    // JSON: volume_id
    #[serde(default, rename = "volume_id")]
    pub ID: String,
    // JSON: type（卷类型）
    #[serde(default, rename = "type")]
    pub Type: String,
    // JSON: snapshot_id
    #[serde(default, rename = "snapshot_id")]
    pub SnapshotID: String,
    // JSON: restore_volume_id
    #[serde(default, rename = "restore_volume_id")]
    pub RestoreVolumeId: String,
    // JSON: volume_az
    #[serde(default, rename = "volume_az")]
    pub VolumeAZ: String,
    // JSON: status
    #[serde(default, rename = "status")]
    pub Status: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
/// 一个 TiKV store 及其挂载卷列表。
pub struct EBSStore {
    // JSON: store_id
    #[serde(default, rename = "store_id")]
    pub StoreID: u64,
    // JSON: volumes
    #[serde(default, rename = "volumes")]
    pub Volumes: Vec<EBSVolume>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
/// TiKV 组件副本与 stores；EBS 路径主要遍历 Stores。
pub struct TiKVComponent {
    // JSON: replicas
    #[serde(default, rename = "replicas")]
    pub Replicas: i32,
    // JSON: stores
    #[serde(default, rename = "stores")]
    pub Stores: Vec<EBSStore>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
/// EBS 备份元数据根：可选 TiKV 组件 + region。
pub struct EBSBasedBRMeta {
    // JSON: tikv 组件
    #[serde(default, rename = "tikv")]
    pub TiKVComponent: Option<TiKVComponent>,
    // JSON: AWS region
    #[serde(default, rename = "region")]
    pub Region: String,
}

/// Mirrors `glue.Progress` without pulling the glue crate.
/// 进度回调仅需 IncBy；避免 cmd 层依赖完整 glue。
pub trait Progress: Send + Sync {
    fn IncBy(&self, count: i64);
}

/// Volume id → availability zone map (`VolumeAZs` in Go).
/// CreateSnapshots 成功后填回，供后续按 AZ 操作。
pub type VolumeAZs = HashMap<String, String>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// EC2 标签键值；快照/卷打标用。
pub struct Tag {
    pub Key: String,
    pub Value: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 快照生命周期状态；Pending/Completed/Error 为常用分支。
pub enum SnapshotState {
    #[default]
    // 创建中
    Pending,
    // 可用
    Completed,
    // 失败终态
    Error,
    // 其他原始状态串
    Other(String),
}

#[derive(Clone, Debug, Default)]
/// DescribeSnapshots / CreateSnapshots 返回的快照视图。
pub struct Snapshot {
    // FSR 对应快照
    pub SnapshotId: String,
    // 源卷 id
    pub VolumeId: String,
    // 快照状态
    pub State: SnapshotState,
    // 如 "80%" 的进度文本
    pub Progress: Option<String>,
    // GiB 容量
    pub VolumeSize: Option<i32>,
    // 状态附加信息
    pub StateMessage: Option<String>,
    // 快照标签
    pub Tags: Vec<Tag>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 卷状态；Available 表示可继续后续步骤。
pub enum VolumeState {
    #[default]
    // 卷创建中
    Creating,
    // 卷可用
    Available,
    Other(String),
}

#[derive(Clone, Debug, Default)]
/// 卷挂载信息；用于反查 EC2 instance id。
pub struct VolumeAttachment {
    // 目标实例
    pub InstanceId: Option<String>,
}

#[derive(Clone, Debug, Default)]
/// DescribeVolumes / CreateVolume 相关卷视图。
pub struct Volume {
    pub VolumeId: String,
    // 卷所在 AZ
    pub AvailabilityZone: Option<String>,
    // 卷状态
    pub State: VolumeState,
    // 卷大小 GiB
    pub Size: Option<i32>,
    // 失败项快照 id
    pub SnapshotId: Option<String>,
    // 是否已 FSR
    pub FastRestored: Option<bool>,
    // 挂载点列表
    pub Attachments: Vec<VolumeAttachment>,
}

#[derive(Clone, Debug, Default)]
/// 块设备上的 EBS 卷 id。
pub struct EbsBlockDevice {
    // 新建卷 id
    pub VolumeId: Option<String>,
}

#[derive(Clone, Debug, Default)]
/// 实例块设备映射；用于排除启动盘与非目标数据盘。
pub struct BlockDeviceMapping {
    // 设备名，如 /dev/xvda
    pub DeviceName: Option<String>,
    // 关联 EBS
    pub Ebs: Option<EbsBlockDevice>,
}

#[derive(Clone, Debug, Default)]
/// DescribeInstances 精简视图。
pub struct Instance {
    // 根设备名
    pub RootDeviceName: Option<String>,
    // 全部块设备
    pub BlockDeviceMappings: Vec<BlockDeviceMapping>,
}

#[derive(Clone, Debug, Default)]
/// CreateSnapshots 入参：实例、是否排除启动盘、排除的数据卷。
pub struct InstanceSpecification {
    pub InstanceId: Option<String>,
    // 是否排除启动卷
    pub ExcludeBootVolume: bool,
    // 排除的数据卷 id
    pub ExcludeDataVolumeIds: Vec<String>,
}

#[derive(Clone, Debug, Default)]
/// 创建快照 API 输出。
pub struct CreateSnapshotsOutput {
    // 新建快照列表
    pub Snapshots: Vec<Snapshot>,
}

#[derive(Clone, Debug, Default)]
/// 创建卷 API 输出。
pub struct CreateVolumeOutput {
    pub VolumeId: Option<String>,
}

#[derive(Clone, Debug, Default)]
/// FSR 描述项：快照 id + 状态字符串。
pub struct FastSnapshotRestore {
    pub SnapshotId: String,
    // FSR 状态原文
    pub State: String,
}

#[derive(Clone, Debug, Default)]
/// FSR 失败错误码条目。
pub struct FsrOperationError {
    // FSR 操作错误码
    pub ErrorCode: String,
}

#[derive(Clone, Debug, Default)]
/// Enable/Disable FSR 返回的失败项。
pub struct FsrUnsuccessfulItem {
    pub SnapshotId: Option<String>,
    // 失败原因列表
    pub FastSnapshotRestoreStateErrors: Vec<FsrOperationError>,
}

#[derive(Clone, Debug)]
/// 带 AWS 错误码的 API 错误；用于识别 PendingSnapshotLimitExceeded。
pub struct Ec2ApiError {
    // AWS 错误码
    pub code: String,
    // 错误消息
    pub message: String,
}

// Display 为 `code: message`，供限流错误识别。
impl std::fmt::Display for Ec2ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

// 标准 Error trait 空实现。
impl std::error::Error for Ec2ApiError {}

/// Polling / retry intervals (Go defaults; tests may shrink them).
/// 测试可通过 with_timers 缩短轮询，避免单测过慢。
#[derive(Clone, Debug)]
/// 各类轮询/重试间隔集合。
pub struct EC2SessionTimers {
    // CreateSnapshots 遇配额错误的退避。
    pub pending_snapshot_retry: Duration,
    // WaitSnapshotsCreated 轮询间隔。
    pub snapshot_poll: Duration,
    // WaitVolumesCreated 轮询间隔。
    pub volume_poll: Duration,
    // FSR credit 不足时的等待。
    pub fsr_credit_retry: Duration,
    // FSR 状态轮询间隔。
    pub fsr_state_poll: Duration,
}

// 与 Go 默认间隔对齐的生产默认值。
impl Default for EC2SessionTimers {
    fn default() -> Self {
        Self {
            // 30s 默认
            pending_snapshot_retry: POLLING_PENDING_SNAPSHOT_INTERVAL,
            // 5s
            snapshot_poll: Duration::from_secs(5),
            // 5s
            volume_poll: Duration::from_secs(5),
            // 3 分钟
            fsr_credit_retry: Duration::from_secs(3 * 60),
            // 1 分钟
            fsr_state_poll: Duration::from_secs(60),
        }
    }
}

/// EC2 client surface used by [`EC2Session`].
/// 注入点：生产可接 SDK，单测接假客户端；方法集覆盖本文件调用面。
pub trait Ec2Client: Send + Sync {
    // 按 id 描述卷。
    fn DescribeVolumes(&self, volume_ids: &[String]) -> Result<Vec<Volume>>;
    // 按 id 描述实例。
    fn DescribeInstances(&self, instance_ids: &[String]) -> Result<Vec<Instance>>;
    // 按实例规格创建快照。
    fn CreateSnapshots(
        &self,
        instance_spec: &InstanceSpecification,
        tags: &[Tag],
    ) -> Result<CreateSnapshotsOutput>;
    // 查询快照状态/进度。
    fn DescribeSnapshots(&self, snapshot_ids: &[String]) -> Result<Vec<Snapshot>>;
    // 删除单个快照。
    fn DeleteSnapshot(&self, snapshot_id: &str) -> Result<()>;
    // 对指定 AZ 启用 FSR。
    fn EnableFastSnapshotRestores(
        &self,
        availability_zones: &[String],
        source_snapshot_ids: &[String],
    ) -> Result<Vec<FsrUnsuccessfulItem>>;
    // 对指定 AZ 禁用 FSR。
    fn DisableFastSnapshotRestores(
        &self,
        availability_zones: &[String],
        source_snapshot_ids: &[String],
    ) -> Result<Vec<FsrUnsuccessfulItem>>;
    // 按状态过滤查询 FSR。
    fn DescribeFastSnapshotRestores(
        &self,
        states: &[String],
        availability_zone: &str,
    ) -> Result<Vec<FastSnapshotRestore>>;
    // 从快照创建卷。
    fn CreateVolume(
        &self,
        snapshot_id: &str,
        availability_zone: &str,
        volume_type: &str,
        iops: Option<i32>,
        throughput: Option<i32>,
        encrypted: bool,
        tags: &[Tag],
    ) -> Result<CreateVolumeOutput>;
    // 删除卷。
    fn DeleteVolume(&self, volume_id: &str) -> Result<()>;
}

/// CloudWatch client surface used by [`EC2Session`].
/// 仅查询 FSR credit balance，供 wait_data_fsr_enabled 决策。
pub trait CloudWatchClient: Send + Sync {
    // 查询 FSR credit；可能返回 None。
    fn GetFastSnapshotRestoreCreditsBalance(
        &self,
        snapshot_id: &str,
        availability_zone: &str,
    ) -> Result<Option<f64>>;
}

/// Synchronous adapter around the official AWS EC2 client.
struct AwsEc2Client {
    client: aws_sdk_ec2::Client,
    runtime: Arc<Runtime>,
}

impl Ec2Client for AwsEc2Client {
    fn DescribeVolumes(&self, volume_ids: &[String]) -> Result<Vec<Volume>> {
        let output = self
            .runtime
            .block_on(
                self.client
                    .describe_volumes()
                    .set_volume_ids(Some(volume_ids.to_vec()))
                    .send(),
            )
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(output.volumes().iter().map(volume_from_aws).collect())
    }

    fn DescribeInstances(&self, instance_ids: &[String]) -> Result<Vec<Instance>> {
        let output = self
            .runtime
            .block_on(
                self.client
                    .describe_instances()
                    .set_instance_ids(Some(instance_ids.to_vec()))
                    .send(),
            )
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(output
            .reservations()
            .iter()
            .flat_map(|reservation| reservation.instances())
            .map(instance_from_aws)
            .collect())
    }

    fn CreateSnapshots(
        &self,
        instance_spec: &InstanceSpecification,
        tags: &[Tag],
    ) -> Result<CreateSnapshotsOutput> {
        let specification = aws_ec2_types::InstanceSpecification::builder()
            .set_instance_id(instance_spec.InstanceId.clone())
            .exclude_boot_volume(instance_spec.ExcludeBootVolume)
            .set_exclude_data_volume_ids(Some(instance_spec.ExcludeDataVolumeIds.clone()))
            .build();
        let tag_specification = aws_ec2_types::TagSpecification::builder()
            .resource_type(aws_ec2_types::ResourceType::Snapshot)
            .set_tags(Some(tags.iter().map(tag_to_aws).collect()))
            .build();
        let output = self
            .runtime
            .block_on(
                self.client
                    .create_snapshots()
                    .instance_specification(specification)
                    .copy_tags_from_source(aws_ec2_types::CopyTagsFromSource::Volume)
                    .tag_specifications(tag_specification)
                    .send(),
            )
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(CreateSnapshotsOutput {
            Snapshots: output
                .snapshots()
                .iter()
                .map(|snapshot| Snapshot {
                    SnapshotId: snapshot.snapshot_id().unwrap_or_default().to_string(),
                    VolumeId: snapshot.volume_id().unwrap_or_default().to_string(),
                    State: snapshot_state_from_aws(snapshot.state()),
                    Progress: snapshot.progress().map(str::to_string),
                    VolumeSize: snapshot.volume_size(),
                    Tags: snapshot.tags().iter().map(tag_from_aws).collect(),
                    ..Default::default()
                })
                .collect(),
        })
    }

    fn DescribeSnapshots(&self, snapshot_ids: &[String]) -> Result<Vec<Snapshot>> {
        let output = self
            .runtime
            .block_on(
                self.client
                    .describe_snapshots()
                    .set_snapshot_ids(Some(snapshot_ids.to_vec()))
                    .send(),
            )
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(output.snapshots().iter().map(snapshot_from_aws).collect())
    }

    fn DeleteSnapshot(&self, snapshot_id: &str) -> Result<()> {
        self.runtime
            .block_on(
                self.client
                    .delete_snapshot()
                    .snapshot_id(snapshot_id)
                    .send(),
            )
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(())
    }

    fn EnableFastSnapshotRestores(
        &self,
        availability_zones: &[String],
        source_snapshot_ids: &[String],
    ) -> Result<Vec<FsrUnsuccessfulItem>> {
        let output = self
            .runtime
            .block_on(
                self.client
                    .enable_fast_snapshot_restores()
                    .set_availability_zones(Some(availability_zones.to_vec()))
                    .set_source_snapshot_ids(Some(source_snapshot_ids.to_vec()))
                    .send(),
            )
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(output
            .unsuccessful()
            .iter()
            .map(|item| FsrUnsuccessfulItem {
                SnapshotId: item.snapshot_id().map(str::to_string),
                FastSnapshotRestoreStateErrors: item
                    .fast_snapshot_restore_state_errors()
                    .iter()
                    .map(|error| FsrOperationError {
                        ErrorCode: format!("{:?}", error.error()),
                    })
                    .collect(),
            })
            .collect())
    }

    fn DisableFastSnapshotRestores(
        &self,
        availability_zones: &[String],
        source_snapshot_ids: &[String],
    ) -> Result<Vec<FsrUnsuccessfulItem>> {
        let output = self
            .runtime
            .block_on(
                self.client
                    .disable_fast_snapshot_restores()
                    .set_availability_zones(Some(availability_zones.to_vec()))
                    .set_source_snapshot_ids(Some(source_snapshot_ids.to_vec()))
                    .send(),
            )
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(output
            .unsuccessful()
            .iter()
            .map(|item| FsrUnsuccessfulItem {
                SnapshotId: item.snapshot_id().map(str::to_string),
                FastSnapshotRestoreStateErrors: item
                    .fast_snapshot_restore_state_errors()
                    .iter()
                    .map(|error| FsrOperationError {
                        ErrorCode: format!("{:?}", error.error()),
                    })
                    .collect(),
            })
            .collect())
    }

    fn DescribeFastSnapshotRestores(
        &self,
        states: &[String],
        availability_zone: &str,
    ) -> Result<Vec<FastSnapshotRestore>> {
        let filters = vec![
            aws_ec2_types::Filter::builder()
                .name("state")
                .set_values(Some(states.to_vec()))
                .build(),
            aws_ec2_types::Filter::builder()
                .name("availability-zone")
                .values(availability_zone)
                .build(),
        ];
        let output = self
            .runtime
            .block_on(
                self.client
                    .describe_fast_snapshot_restores()
                    .set_filters(Some(filters))
                    .send(),
            )
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(output
            .fast_snapshot_restores()
            .iter()
            .map(|item| FastSnapshotRestore {
                SnapshotId: item.snapshot_id().unwrap_or_default().to_string(),
                State: item
                    .state()
                    .map(|state| state.as_str().to_string())
                    .unwrap_or_default(),
            })
            .collect())
    }

    fn CreateVolume(
        &self,
        snapshot_id: &str,
        availability_zone: &str,
        volume_type: &str,
        iops: Option<i32>,
        throughput: Option<i32>,
        encrypted: bool,
        tags: &[Tag],
    ) -> Result<CreateVolumeOutput> {
        let tag_specification = aws_ec2_types::TagSpecification::builder()
            .resource_type(aws_ec2_types::ResourceType::Volume)
            .set_tags(Some(tags.iter().map(tag_to_aws).collect()))
            .build();
        let output = self
            .runtime
            .block_on(
                self.client
                    .create_volume()
                    .snapshot_id(snapshot_id)
                    .availability_zone(availability_zone)
                    .volume_type(aws_ec2_types::VolumeType::from(volume_type))
                    .set_iops(iops)
                    .set_throughput(throughput)
                    .encrypted(encrypted)
                    .tag_specifications(tag_specification)
                    .send(),
            )
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(CreateVolumeOutput {
            VolumeId: output.volume_id().map(str::to_string),
        })
    }

    fn DeleteVolume(&self, volume_id: &str) -> Result<()> {
        self.runtime
            .block_on(self.client.delete_volume().volume_id(volume_id).send())
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(())
    }
}

/// Synchronous adapter around the official CloudWatch client.
struct AwsCloudWatchClient {
    client: aws_sdk_cloudwatch::Client,
    runtime: Arc<Runtime>,
}

impl CloudWatchClient for AwsCloudWatchClient {
    fn GetFastSnapshotRestoreCreditsBalance(
        &self,
        snapshot_id: &str,
        availability_zone: &str,
    ) -> Result<Option<f64>> {
        let end_time = SystemTime::now();
        let start_time = end_time - Duration::from_secs(5 * 60);
        let dimensions = vec![
            Dimension::builder()
                .name("SnapshotId")
                .value(snapshot_id)
                .build(),
            Dimension::builder()
                .name("AvailabilityZone")
                .value(availability_zone)
                .build(),
        ];
        let output = self
            .runtime
            .block_on(
                self.client
                    .get_metric_statistics()
                    .start_time(start_time.into())
                    .end_time(end_time.into())
                    .namespace("AWS/EBS")
                    .metric_name("FastSnapshotRestoreCreditsBalance")
                    .set_dimensions(Some(dimensions))
                    .period(300)
                    .statistics(Statistic::Maximum)
                    .send(),
            )
            .map_err(|err| aws_error_to_string(&err))?;
        Ok(output
            .datapoints()
            .first()
            .and_then(|point| point.maximum()))
    }
}

fn tag_to_aws(tag: &Tag) -> aws_ec2_types::Tag {
    aws_ec2_types::Tag::builder()
        .key(&tag.Key)
        .value(&tag.Value)
        .build()
}

/// Render an AWS SDK error without dropping service metadata.
pub(crate) fn aws_error_to_string(
    error: &(impl std::fmt::Display + ProvideErrorMetadata),
) -> String {
    match (error.code(), error.message()) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_string(),
        (None, _) => error.to_string(),
    }
}

fn tag_from_aws(tag: &aws_ec2_types::Tag) -> Tag {
    Tag {
        Key: tag.key().unwrap_or_default().to_string(),
        Value: tag.value().unwrap_or_default().to_string(),
    }
}

fn snapshot_state_from_aws(state: Option<&aws_ec2_types::SnapshotState>) -> SnapshotState {
    match state.map(aws_ec2_types::SnapshotState::as_str) {
        Some("pending") => SnapshotState::Pending,
        Some("completed") => SnapshotState::Completed,
        Some("error") => SnapshotState::Error,
        Some(other) => SnapshotState::Other(other.to_string()),
        None => SnapshotState::Other(String::new()),
    }
}

fn snapshot_from_aws(snapshot: &aws_ec2_types::Snapshot) -> Snapshot {
    Snapshot {
        SnapshotId: snapshot.snapshot_id().unwrap_or_default().to_string(),
        VolumeId: snapshot.volume_id().unwrap_or_default().to_string(),
        State: snapshot_state_from_aws(snapshot.state()),
        Progress: snapshot.progress().map(str::to_string),
        VolumeSize: snapshot.volume_size(),
        StateMessage: snapshot.state_message().map(str::to_string),
        Tags: snapshot.tags().iter().map(tag_from_aws).collect(),
    }
}

fn volume_from_aws(volume: &aws_ec2_types::Volume) -> Volume {
    Volume {
        VolumeId: volume.volume_id().unwrap_or_default().to_string(),
        AvailabilityZone: volume.availability_zone().map(str::to_string),
        State: match volume.state().map(aws_ec2_types::VolumeState::as_str) {
            Some("available") => VolumeState::Available,
            Some("creating") => VolumeState::Creating,
            Some(other) => VolumeState::Other(other.to_string()),
            None => VolumeState::Other(String::new()),
        },
        Size: volume.size(),
        SnapshotId: volume.snapshot_id().map(str::to_string),
        FastRestored: volume.fast_restored(),
        Attachments: volume
            .attachments()
            .iter()
            .map(|attachment| VolumeAttachment {
                InstanceId: attachment.instance_id().map(str::to_string),
            })
            .collect(),
    }
}

fn instance_from_aws(instance: &aws_ec2_types::Instance) -> Instance {
    Instance {
        RootDeviceName: instance.root_device_name().map(str::to_string),
        BlockDeviceMappings: instance
            .block_device_mappings()
            .iter()
            .map(|mapping| BlockDeviceMapping {
                DeviceName: mapping.device_name().map(str::to_string),
                Ebs: mapping.ebs().map(|ebs| EbsBlockDevice {
                    VolumeId: ebs.volume_id().map(str::to_string),
                }),
            })
            .collect(),
    }
}

/// EBS 操作会话：持有客户端、并发度与计时器。
pub struct EC2Session {
    // 注入的 EC2 客户端。
    ec2: Arc<dyn Ec2Client>,
    // 注入的 CloudWatch 客户端。
    cloudwatch: Arc<dyn CloudWatchClient>,
    // WorkerPool 并发度。
    concurrency: u32,
    // 可覆盖的轮询参数。
    timers: EC2SessionTimers,
}

impl EC2Session {
    /// 测试/注入入口：使用给定 Ec2/CloudWatch 客户端构造会话。
    pub fn NewEC2SessionWithClients(
        concurrency: u32,
        ec2: Arc<dyn Ec2Client>,
        cloudwatch: Arc<dyn CloudWatchClient>,
    ) -> Self {
        Self {
            ec2,
            cloudwatch,
            concurrency,
            timers: EC2SessionTimers::default(),
        }
    }

    // 覆盖默认轮询间隔（单测常用）。
    pub fn with_timers(mut self, timers: EC2SessionTimers) -> Self {
        self.timers = timers;
        self
    }
}

/// Load the default AWS configuration and construct live EC2/CloudWatch clients.
pub fn NewEC2Session(concurrency: u32, region: &str) -> Result<EC2Session> {
    let runtime =
        Arc::new(Runtime::new().map_err(|err| format!("failed to create AWS runtime: {err}"))?);
    // Use the compiled-in Mozilla roots so client construction does not depend on a
    // readable platform trust store. TLS verification remains enabled for AWS endpoints.
    let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    let http_client =
        aws_smithy_http_client::hyper_014::HyperClientBuilder::new().build(https_connector);
    let config = runtime.block_on(
        aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region.to_string()))
            .retry_config(RetryConfig::standard().with_max_attempts(9))
            .http_client(http_client)
            .load(),
    );
    let ec2 = Arc::new(AwsEc2Client {
        client: aws_sdk_ec2::Client::new(&config),
        runtime: Arc::clone(&runtime),
    });
    let cloudwatch = Arc::new(AwsCloudWatchClient {
        client: aws_sdk_cloudwatch::Client::new(&config),
        runtime,
    });
    Ok(EC2Session::NewEC2SessionWithClients(
        concurrency,
        ec2,
        cloudwatch,
    ))
}

impl EC2Session {
    /// 按 meta 中各 store 的卷创建快照。
    /// 数据流：DescribeVolumes→找挂载实例→DescribeInstances→排除非目标盘→
    /// WorkerPool 并发 CreateSnapshots（含 pending-limit 重试）→汇总 volume→snapshot，
    /// 再 DescribeVolumes 填充 VolumeAZs。任一步失败返回已收集的 map + Err。
    pub fn CreateSnapshots(
        &self,
        backupInfo: &EBSBasedBRMeta,
    ) -> (HashMap<String, String>, VolumeAZs, Result<()>) {
        // volumeId → snapshotId，跨 worker 填充。
        let snap_id_map = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        let mut volume_ids = Vec::new();

        // 收集并发首错。
        let eg = ErrorGroup::new();
        let fill_result = {
            let snap_id_map = Arc::clone(&snap_id_map);
            move |create_output: &CreateSnapshotsOutput| {
                let mut guard = snap_id_map.lock().expect("snap mutex poisoned");
                for snapshot in &create_output.Snapshots {
                    guard.insert(snapshot.VolumeId.clone(), snapshot.SnapshotId.clone());
                }
            }
        };

        // BR 快照统一打标，便于清理与识别。
        let tags = vec![ec2_tag("TiDBCluster-BR-Snapshot", "new")];
        // concurrency 至少为 1，避免零宽池。
        let worker_pool = WorkerPool::new(self.concurrency.max(1) as usize);

        let stores = tikv_stores(backupInfo);
        for store in stores {
            let volumes = &store.Volumes;
            // 无卷的 store 跳过。
            if volumes.is_empty() {
                continue;
            }

            let mut target_volume_ids = Vec::new();
            for volume in volumes {
                target_volume_ids.push(volume.ID.clone());
                volume_ids.push(volume.ID.clone());
            }

            // 用首个卷反查挂载实例（与 Go 相同取巧）。
            let resp = match self.ec2.DescribeVolumes(&target_volume_ids[0..1]) {
                Ok(volumes) => volumes,
                Err(err) => {
                    return (
                        snapshot_map_from_mutex(&snap_id_map),
                        VolumeAZs::new(),
                        Err(err),
                    );
                }
            };
            let Some(first_volume) = resp.first() else {
                return (
                    snapshot_map_from_mutex(&snap_id_map),
                    VolumeAZs::new(),
                    Err("DescribeVolumes returned no volumes".into()),
                );
            };
            // 未挂载则无法 CreateSnapshots(instance)。
            let Some(first_attachment) = first_volume.Attachments.first() else {
                return (
                    snapshot_map_from_mutex(&snap_id_map),
                    VolumeAZs::new(),
                    Err(format!(
                        "specified volume {} is not attached",
                        volumes[0].ID
                    )),
                );
            };
            let Some(ec2_instance_id) = first_attachment.InstanceId.clone() else {
                return (
                    snapshot_map_from_mutex(&snap_id_map),
                    VolumeAZs::new(),
                    Err(format!(
                        "specified volume {} is not attached",
                        volumes[0].ID
                    )),
                );
            };

            let resp1 = match self.ec2.DescribeInstances(&[ec2_instance_id.clone()]) {
                Ok(instances) => instances,
                Err(err) => {
                    return (
                        snapshot_map_from_mutex(&snap_id_map),
                        VolumeAZs::new(),
                        Err(err),
                    );
                }
            };
            let Some(first_instance) = resp1.first() else {
                return (
                    snapshot_map_from_mutex(&snap_id_map),
                    VolumeAZs::new(),
                    Err("DescribeInstances returned no instances".into()),
                );
            };

            // 实例上非目标数据盘需 Exclude，避免误快照。
            let mut excluded_volume_ids = Vec::new();
            for device in &first_instance.BlockDeviceMappings {
                let device_name = device.DeviceName.clone().unwrap_or_default();
                let root_device_name = first_instance.RootDeviceName.clone().unwrap_or_default();
                // 启动盘由 ExcludeBootVolume 处理，此处跳过收集。
                if device_name == root_device_name {
                    continue;
                }
                let mut to_include = false;
                let ebs_volume_id = device
                    .Ebs
                    .as_ref()
                    .and_then(|ebs| ebs.VolumeId.clone())
                    .unwrap_or_default();
                for target_volume_id in &target_volume_ids {
                    if target_volume_id == &ebs_volume_id {
                        to_include = true;
                        break;
                    }
                }
                if !to_include {
                    if let Some(ebs) = &device.Ebs {
                        if let Some(volume_id) = &ebs.VolumeId {
                            if !volume_id.is_empty() {
                                excluded_volume_ids.push(volume_id.clone());
                            }
                        }
                    }
                }
            }

            let ec2 = Arc::clone(&self.ec2);
            let timers = self.timers.clone();
            let instance_spec = InstanceSpecification {
                InstanceId: Some(ec2_instance_id),
                // 始终排除启动盘（BR 只关心数据盘）
                ExcludeBootVolume: true,
                ExcludeDataVolumeIds: excluded_volume_ids,
            };
            let tags_for_task = tags.clone();
            // 闭包克隆以写入共享 map
            let fill_result = fill_result.clone();
            // 每 store 一个创建任务，受池并发限制。
            worker_pool.apply_on_error_group(&eg, move || {
                let resp =
                    create_snapshots_with_retry(&ec2, &instance_spec, &tags_for_task, &timers)?;
                fill_result(&resp);
                Ok(())
            });
        }

        // 创建阶段失败：仍返回已填 map，AZ 为空。
        if let Err(err) = eg.wait() {
            return (
                snapshot_map_from_mutex(&snap_id_map),
                VolumeAZs::new(),
                Err(err),
            );
        }

        // 二次 DescribeVolumes 收集 AZ。
        let mut vol_azs = VolumeAZs::new();
        let resp = match self.ec2.DescribeVolumes(&volume_ids) {
            Ok(volumes) => volumes,
            Err(err) => {
                return (snapshot_map_from_mutex(&snap_id_map), vol_azs, Err(err));
            }
        };
        for vol in resp {
            if !vol.VolumeId.is_empty() {
                if let Some(availability_zone) = &vol.AvailabilityZone {
                    if !availability_zone.is_empty() {
                        vol_azs.insert(vol.VolumeId.clone(), availability_zone.clone());
                    }
                }
            }
        }

        // 成功：返回映射与 AZ
        (snapshot_map_from_mutex(&snap_id_map), vol_azs, Ok(()))
    }

    /// 轮询直到全部快照 Completed；累加 VolumeSize，并按进度百分比增量上报。
    /// 任一快照 Error 立即失败；Pending 继续下一轮。
    pub fn WaitSnapshotsCreated(
        &self,
        snap_id_map: HashMap<String, String>,
        progress: &dyn Progress,
    ) -> Result<i64> {
        // 待完成快照 id 列表，随轮询收缩。
        let mut pending_snapshots: Vec<String> = snap_id_map.values().cloned().collect();
        let mut total_volume_size: i64 = 0;
        // 记录已上报进度，仅增量 IncBy。
        let mut snap_progress_map: HashMap<String, i64> = HashMap::with_capacity(snap_id_map.len());

        loop {
            // FSR 状态阶段完成
            if pending_snapshots.is_empty() {
                return Ok(total_volume_size);
            }

            // 轮询间隔
            std::thread::sleep(self.timers.snapshot_poll);
            let resp = self.ec2.DescribeSnapshots(&pending_snapshots)?;

            // 下一轮继续等待的 id
            let mut uncompleted_snapshots = Vec::new();
            for mut snapshot in resp {
                let snapshot_id = snapshot.SnapshotId.clone();
                match &snapshot.State {
                    // 完成则计入总容量。
                    SnapshotState::Completed => {
                        if let Some(volume_size) = snapshot.VolumeSize {
                            total_volume_size += i64::from(volume_size);
                        }
                    }
                    // 快照失败不可恢复。
                    SnapshotState::Error => {
                        return Err(format!("snapshot {snapshot_id} failed"));
                    }
                    _ => {
                        if !snapshot_id.is_empty() {
                            uncompleted_snapshots.push(snapshot_id.clone());
                        }
                    }
                }
                // 进度字符串可能被 trim 写回。
                let curr_snap_progress = self.extract_snap_progress(snapshot.Progress.as_mut());
                let previous = snap_progress_map.get(&snapshot_id).copied().unwrap_or(0);
                if curr_snap_progress > previous {
                    progress.IncBy(curr_snap_progress - previous);
                    snap_progress_map.insert(snapshot_id, curr_snap_progress);
                }
            }
            pending_snapshots = uncompleted_snapshots;
        }
    }

    /// 尽力删除映射中的快照；单删失败不中断，等待汇聚后忽略首错。
    pub fn DeleteSnapshots(&self, snap_id_map: HashMap<String, String>) {
        let pending_snaps: Vec<String> = snap_id_map.values().cloned().collect();
        // 仅统计成功删除数，不向外返回
        let deleted_cnt = Arc::new(Mutex::new(0_i32));
        let eg = ErrorGroup::new();
        let worker_pool = WorkerPool::new(self.concurrency.max(1) as usize);
        for snap_id in pending_snaps {
            let ec2 = Arc::clone(&self.ec2);
            let deleted_cnt = Arc::clone(&deleted_cnt);
            worker_pool.apply_on_error_group(&eg, move || {
                if ec2.DeleteSnapshot(&snap_id).is_ok() {
                    *deleted_cnt.lock().expect("deleted cnt poisoned") += 1;
                }
                Ok(())
            });
        }
        // 删除路径吞掉错误（尽力而为）
        let _ = eg.wait();
    }

    /// 按目标 AZ 启用数据盘 FSR：分批（≤FsrApiSnapshotsThreshold）调用 API，
    /// 再 wait_data_fsr_enabled 等待 credit 与状态就绪。
    /// 返回 az→snapshotIds 映射及整体 Result。
    pub fn EnableDataFSR(
        &self,
        meta: &EBSBasedBRMeta,
        target_az: &str,
    ) -> (HashMap<String, Vec<String>>, Result<()>) {
        // 按 AZ 归类目标快照；空 meta 视为错误。
        let snapshots_ids_map = fetch_target_snapshots(meta, target_az);
        // 空输入：无需禁用
        if snapshots_ids_map.is_empty() {
            return (snapshots_ids_map, Err("empty backup meta".into()));
        }

        let eg = ErrorGroup::new();
        for (available_zone, snapshots) in &snapshots_ids_map {
            let target_az_key = available_zone.clone();
            let mut i = 0;
            while i < snapshots.len() {
                let start = i;
                // 批大小不超过 API 阈值。
                let end = (i + FsrApiSnapshotsThreshold).min(snapshots.len());
                let batch = snapshots[start..end].to_vec();
                let ec2 = Arc::clone(&self.ec2);
                let cloudwatch = Arc::clone(&self.cloudwatch);
                let session = EC2Session {
                    ec2,
                    cloudwatch,
                    concurrency: self.concurrency,
                    timers: self.timers.clone(),
                };
                let target_az_for_task = target_az_key.clone();
                // FSR 批任务直接进 ErrorGroup（不受 WorkerPool）
                eg.go(move || {
                    let unsuccessful = session
                        .ec2
                        .EnableFastSnapshotRestores(&[target_az_for_task.clone()], &batch)?;
                    if !unsuccessful.is_empty() {
                        let snap = unsuccessful[0].SnapshotId.clone().unwrap_or_default();
                        return Err(format!(
                            "Some snapshot fails to enable FSR for available zone {}, such as {}, error code is {:?}",
                            target_az_for_task,
                            snap,
                            unsuccessful[0].FastSnapshotRestoreStateErrors
                        ));
                    }
                    session.wait_data_fsr_enabled(batch, &target_az_for_task)
                });
                i += FsrApiSnapshotsThreshold;
            }
        }
        (snapshots_ids_map, eg.wait())
    }

    /// 对称地分批禁用 FSR；空 map 直接成功。
    pub fn DisableDataFSR(&self, snapshots_ids_map: HashMap<String, Vec<String>>) -> Result<()> {
        if snapshots_ids_map.is_empty() {
            return Ok(());
        }

        let eg = ErrorGroup::new();
        for (available_zone, snapshots) in &snapshots_ids_map {
            let mut i = 0;
            while i < snapshots.len() {
                let start = i;
                let end = (i + FsrApiSnapshotsThreshold).min(snapshots.len());
                let batch = snapshots[start..end].to_vec();
                let ec2 = Arc::clone(&self.ec2);
                let target_az = available_zone.clone();
                eg.go(move || {
                    let unsuccessful =
                        ec2.DisableFastSnapshotRestores(&[target_az.clone()], &batch)?;
                    if !unsuccessful.is_empty() {
                        let snap = unsuccessful[0].SnapshotId.clone().unwrap_or_default();
                        return Err(format!(
                            "Some snapshot fails to disable FSR for available zone {}, such as {}, error code is {:?}",
                            target_az,
                            snap,
                            unsuccessful[0].FastSnapshotRestoreStateErrors
                        ));
                    }
                    Ok(())
                });
                i += FsrApiSnapshotsThreshold;
            }
        }
        eg.wait()
    }

    /// 从元数据快照创建新卷：并发 CreateVolume，复制/改写标签，
    /// 返回 oldVolumeId→newVolumeId；iops/throughput≤0 视为未指定。
    pub fn CreateVolumes(
        &self,
        meta: &EBSBasedBRMeta,
        volume_type: &str,
        iops: i64,
        throughput: i64,
        encrypted: bool,
        target_az: &str,
    ) -> (HashMap<String, String>, Result<()>) {
        // ≤0 表示调用方未指定，透传 None 给 API。
        let iops_opt = if iops > 0 { Some(iops as i32) } else { None };
        let throughput_opt = if throughput > 0 {
            Some(throughput as i32)
        } else {
            None
        };

        // 旧卷 id → 新卷 id。
        let new_volume_id_map = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        let eg = ErrorGroup::new();

        let worker_pool = WorkerPool::new(self.concurrency.max(1) as usize);
        let stores = tikv_stores(meta);
        for store in stores {
            for old_vol in &store.Volumes {
                let old_vol = old_vol.clone();
                let ec2 = Arc::clone(&self.ec2);
                let volume_type = volume_type.to_string();
                let target_az = target_az.to_string();
                let new_volume_id_map = Arc::clone(&new_volume_id_map);
                worker_pool.apply_on_error_group(&eg, move || {
                    // 未指定目标 AZ 时沿用原卷 AZ。
                    let availability_zone = if target_az.is_empty() {
                        old_vol.VolumeAZ.clone()
                    } else {
                        target_az.clone()
                    };

                    // CSI/BR 识别标签 + 来源快照 id；并继承非 snapshot/ 前缀的源标签。
                    let mut tags = vec![
                        ec2_tag("TiDBCluster-BR", "new"),
                        ec2_tag("ebs.csi.aws.com/cluster", "true"),
                        ec2_tag("snapshot/createdFromSnapshotId", &old_vol.SnapshotID),
                    ];

                    let snapshots = ec2.DescribeSnapshots(&[old_vol.SnapshotID.clone()])?;
                    // 源快照必须存在
                    if snapshots.is_empty() {
                        return Err(format!(
                            "specified snapshot [{}] is not found",
                            old_vol.SnapshotID
                        ));
                    }

                    // 继承标签时加 snapshot/ 前缀，避免与系统键冲突
                    for source_tag in &snapshots[0].Tags {
                        if !source_tag.Key.starts_with("snapshot/") {
                            tags.push(ec2_tag(
                                &format!("snapshot/{}", source_tag.Key),
                                &source_tag.Value,
                            ));
                        }
                    }

                    let new_vol = ec2.CreateVolume(
                        &old_vol.SnapshotID,
                        &availability_zone,
                        &volume_type,
                        iops_opt,
                        throughput_opt,
                        encrypted,
                        &tags,
                    )?;
                    if let Some(volume_id) = &new_vol.VolumeId {
                        if !volume_id.is_empty() {
                            new_volume_id_map
                                .lock()
                                .expect("volume mutex poisoned")
                                .insert(old_vol.ID.clone(), volume_id.clone());
                        }
                    }
                    Ok(())
                });
            }
        }
        // 先 wait 再取出 map，保证写入完成
        let result = eg.wait();
        (volume_map_from_mutex(&new_volume_id_map), result)
    }

    /// 轮询卷变为 Available；可选要求 FastRestored；进度按本轮完成数 IncBy。
    pub fn WaitVolumesCreated(
        &self,
        volume_id_map: HashMap<String, String>,
        progress: &dyn Progress,
        fsr_enabled_required: bool,
    ) -> Result<i64> {
        let mut pending_volumes: Vec<String> = volume_id_map.values().cloned().collect();
        let mut total_volume_size: i64 = 0;

        // 直到全部 Available（或 FSR 校验失败）。
        while !pending_volumes.is_empty() {
            std::thread::sleep(self.timers.volume_poll);
            let resp = self.ec2.DescribeVolumes(&pending_volumes)?;
            let describe_output = DescribeVolumesOutput { Volumes: resp };
            let (created_volume_size, unfinished_volumes) =
                self.HandleDescribeVolumesResponse(&describe_output, fsr_enabled_required)?;
            // 本轮新完成的卷数
            progress.IncBy((pending_volumes.len() - unfinished_volumes.len()) as i64);
            total_volume_size += created_volume_size;
            pending_volumes = unfinished_volumes;
        }
        Ok(total_volume_size)
    }

    /// 尽力删除新卷；语义同 DeleteSnapshots。
    pub fn DeleteVolumes(&self, volume_id_map: HashMap<String, String>) {
        let pending_volumes: Vec<String> = volume_id_map.values().cloned().collect();
        let deleted_cnt = Arc::new(Mutex::new(0_i32));
        let eg = ErrorGroup::new();
        let worker_pool = WorkerPool::new(self.concurrency.max(1) as usize);
        for vol_id in pending_volumes {
            let ec2 = Arc::clone(&self.ec2);
            let deleted_cnt = Arc::clone(&deleted_cnt);
            worker_pool.apply_on_error_group(&eg, move || {
                if ec2.DeleteVolume(&vol_id).is_ok() {
                    *deleted_cnt.lock().expect("deleted cnt poisoned") += 1;
                }
                Ok(())
            });
        }
        let _ = eg.wait();
    }

    /// 解析 DescribeVolumes：Available 累加 size，其余列入未完成；
    /// 若要求 FSR 且 FastRestored 明确为 false 则报错。
    pub fn HandleDescribeVolumesResponse(
        &self,
        resp: &DescribeVolumesOutput,
        fsr_enabled_required: bool,
    ) -> Result<(i64, Vec<String>)> {
        let mut total_volume_size: i64 = 0;
        let mut unfinished_volumes = Vec::new();
        for volume in &resp.Volumes {
            let volume_id = volume.VolumeId.clone();
            if volume.State == VolumeState::Available {
                // 要求 FSR 时校验 FastRestored
                if fsr_enabled_required
                    && volume.FastRestored.is_some()
                    && !volume.FastRestored.unwrap_or(false)
                {
                    let snapshot_id = volume.SnapshotId.clone().unwrap_or_default();
                    return Err(format!(
                        "Snapshot [{snapshot_id}] of volume [{volume_id}] is not fsr enabled"
                    ));
                }
                if let Some(size) = volume.Size {
                    total_volume_size += i64::from(size);
                }
            // 非 Available：继续等待
            } else if !volume_id.is_empty() {
                unfinished_volumes.push(volume_id);
            }
        }
        Ok((total_volume_size, unfinished_volumes))
    }

    /// 解析 AWS 进度字符串（如 "42%"）为 0..=100 整数；非法则 0。
    /// 会 trim 写回传入的 String。
    pub(crate) fn extract_snap_progress(&self, str_value: Option<&mut String>) -> i64 {
        // 无进度字段
        let Some(str_value) = str_value else {
            return 0;
        };
        let trimmed = str_value.trim_matches(' ').to_string();
        *str_value = trimmed.clone();
        // Go fmt.Sscanf("%f%%") accepts trailing input after the first literal `%`.
        let Some(percent_index) = trimmed.find('%') else {
            return 0;
        };
        let number_part = trimmed[..percent_index].trim();
        let mut val = match number_part.parse::<f64>() {
            Ok(val) => val,
            Err(_) => return 0,
        };
        // 防御性截断异常进度值。
        if val > 100.0 {
            val = 100.0;
        }
        val as i64
    }

    /// 等待批次 FSR 可用：先按 credit≥1 推进索引（对齐 Go 忽略部分 CW 错误），
    /// 再轮询 DescribeFastSnapshotRestores，disabled/disabling 视为失败。
    fn wait_data_fsr_enabled(&self, snap_shot_ids: Vec<String>, target_az: &str) -> Result<()> {
        let resp = self.ec2.DescribeSnapshots(&snap_shot_ids)?;
        // 快照不存在
        if resp.is_empty() {
            return Err(format!(
                "specified snapshot [{}] is not found",
                snap_shot_ids[0]
            ));
        }

        // 按 credit 逐个放行快照索引。
        let mut start_idx = 0usize;
        let mut retry_count = 0usize;
        while start_idx < snap_shot_ids.len() {
            // Go ignores getFSRCreditBalance error and treats it as nil balance.
            // 与 Go 一致：查询失败当 nil balance，靠重试计数保护。
            let credit_balance = self
                .get_fsr_credit_balance(&snap_shot_ids[start_idx], target_az)
                .ok()
                .flatten();
            // credit 足够则处理下一快照
            if credit_balance.is_some_and(|balance| balance >= 1.0) {
                start_idx += 1;
                retry_count = 0;
            } else {
                // 连续失败超过 3 次才报错
                if credit_balance.is_none() {
                    if retry_count >= 3 {
                        return Err(format!(
                            "cloudwatch metrics for {} operation failed after retrying",
                            snap_shot_ids[start_idx]
                        ));
                    }
                    retry_count += 1;
                }
                std::thread::sleep(self.timers.fsr_credit_retry);
            }
        }

        // 第二阶段：等待 FSR 状态离开 enabling/optimizing 集合中的“未完成”项。
        let mut pending_snapshots: HashSet<String> = snap_shot_ids.iter().cloned().collect();
        loop {
            if pending_snapshots.is_empty() {
                return Ok(());
            }

            // 状态轮询
            std::thread::sleep(self.timers.fsr_state_poll);
            let result = self.ec2.DescribeFastSnapshotRestores(
                &[
                    "disabled".into(),
                    "disabling".into(),
                    "enabling".into(),
                    "optimizing".into(),
                ],
                target_az,
            )?;

            // 仍处于启用过程的快照
            let mut uncompleted_snapshots = HashSet::new();
            for fast_restore in result {
                let snapshot_id = fast_restore.SnapshotId.clone();
                if pending_snapshots.contains(&snapshot_id) {
                    let state_str = fast_restore.State.clone();
                    // 禁用态表示启用失败。
                    if equal_fold(&state_str, "disabled") || equal_fold(&state_str, "disabling") {
                        return Err(format!("status of snapshot {snapshot_id} is {state_str} "));
                    }
                    uncompleted_snapshots.insert(snapshot_id);
                }
            }
            pending_snapshots = uncompleted_snapshots;
        }
    }

    // 透传 CloudWatch 查询。
    fn get_fsr_credit_balance(&self, snapshot_id: &str, target_az: &str) -> Result<Option<f64>> {
        self.cloudwatch
            .GetFastSnapshotRestoreCreditsBalance(snapshot_id, target_az)
    }
}

#[derive(Clone, Debug, Default)]
/// DescribeVolumes 包装，便于与 Go 输出结构对照。
pub struct DescribeVolumesOutput {
    pub Volumes: Vec<Volume>,
}

/// 创建快照并在 PendingSnapshotLimitExceeded 时按 pending_snapshot_retry 退避重试。
/// 其他错误立即返回。
fn create_snapshots_with_retry(
    ec2: &Arc<dyn Ec2Client>,
    instance_spec: &InstanceSpecification,
    tags: &[Tag],
    timers: &EC2SessionTimers,
) -> Result<CreateSnapshotsOutput> {
    loop {
        match ec2.CreateSnapshots(instance_spec, tags) {
            Ok(res) => return Ok(res),
            Err(err) => {
                if is_pending_snapshot_limit_exceeded(&err) {
                    std::thread::sleep(timers.pending_snapshot_retry);
                    continue;
                }
                return Err(format!(
                    "failed to create snapshot for request {instance_spec:?}: {err}"
                ));
            }
        }
    }
}

/// 从元数据收集 `storage.data-dir` 快照并按 AZ 分组（Go：`fetchTargetSnapshots`）。
/// `specified_az` 非空时全部归入该 AZ，否则用各卷自身 `VolumeAZ`。
pub(crate) fn fetch_target_snapshots(
    meta: &EBSBasedBRMeta,
    specified_az: &str,
) -> HashMap<String, Vec<String>> {
    let mut source_snapshot_ids = HashMap::new();
    if meta
        .TiKVComponent
        .as_ref()
        .is_none_or(|c| c.Stores.is_empty())
    {
        return source_snapshot_ids;
    }
    for store in tikv_stores(meta) {
        for old_vol in &store.Volumes {
            if old_vol.Type == "storage.data-dir" {
                if !specified_az.is_empty() {
                    source_snapshot_ids
                        .entry(specified_az.to_string())
                        .or_default()
                        .push(old_vol.SnapshotID.clone());
                } else {
                    source_snapshot_ids
                        .entry(old_vol.VolumeAZ.clone())
                        .or_default()
                        .push(old_vol.SnapshotID.clone());
                }
            }
        }
    }
    source_snapshot_ids
}

/// 构造 Tag 辅助函数。
fn ec2_tag(key: &str, val: &str) -> Tag {
    Tag {
        Key: key.to_string(),
        Value: val.to_string(),
    }
}

/// 安全取出 TiKV stores 切片；缺省返回空。
fn tikv_stores(meta: &EBSBasedBRMeta) -> &[EBSStore] {
    meta.TiKVComponent
        .as_ref()
        .map(|c| c.Stores.as_slice())
        .unwrap_or(&[])
}

/// 从互斥 map 克隆快照映射。
fn snapshot_map_from_mutex(
    snap_id_map: &Arc<Mutex<HashMap<String, String>>>,
) -> HashMap<String, String> {
    snap_id_map.lock().expect("snap mutex poisoned").clone()
}

/// 从互斥 map 克隆卷映射。
fn volume_map_from_mutex(
    volume_id_map: &Arc<Mutex<HashMap<String, String>>>,
) -> HashMap<String, String> {
    volume_id_map.lock().expect("volume mutex poisoned").clone()
}

/// ASCII 大小写不敏感比较（FSR 状态字符串）。
fn equal_fold(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// 识别 Pending 快照配额错误（子串或 Ec2ApiError Display）。
fn is_pending_snapshot_limit_exceeded(err: &str) -> bool {
    err.contains(ERR_CODE_TOO_MANY_PENDING_SNAPSHOTS)
}

/// First-error join group mirroring Go `errgroup`.
/// 首错 errgroup 替身：go 提交任务，wait 汇聚首个 Err。
struct ErrorGroup {
    first_err: Arc<Mutex<Option<String>>>,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl ErrorGroup {
    // 空组。
    fn new() -> Self {
        Self {
            first_err: Arc::new(Mutex::new(None)),
            handles: Mutex::new(Vec::new()),
        }
    }

    // 立即开线程执行；首错写入 shared。
    fn go<F>(&self, f: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let first_err = Arc::clone(&self.first_err);
        let handle = std::thread::spawn(move || {
            if let Err(err) = f() {
                let mut guard = first_err.lock().expect("first_err poisoned");
                if guard.is_none() {
                    *guard = Some(err);
                }
            }
        });
        self.handles.lock().expect("handles poisoned").push(handle);
    }

    // 等待全部句柄后返回首错或 Ok。
    fn wait(self) -> Result<()> {
        let handles = self.handles.into_inner().expect("handles poisoned");
        for handle in handles {
            let _ = handle.join();
        }
        match Arc::try_unwrap(self.first_err)
            .expect("first_err still shared")
            .into_inner()
            .expect("first_err poisoned")
        {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

/// Bounded worker pool mirroring Go `util.WorkerPool` + `ApplyOnErrorGroup`.
/// 有界工作池：通过信号量限制并发，任务仍走 ErrorGroup。
struct WorkerPool {
    concurrency: usize,
    active: Arc<(Mutex<usize>, Condvar)>,
}

impl WorkerPool {
    // concurrency 为同时运行任务上限。
    fn new(concurrency: usize) -> Self {
        Self {
            concurrency: concurrency.max(1),
            active: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }

    // 获取令牌后 eg.go；任务结束释放令牌。
    fn apply_on_error_group<F>(&self, eg: &ErrorGroup, f: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        {
            let (lock, cvar) = &*self.active;
            let mut n = lock.lock().expect("worker pool poisoned");
            while *n >= self.concurrency {
                n = cvar.wait(n).expect("worker pool wait poisoned");
            }
            *n += 1;
        }
        let active = Arc::clone(&self.active);
        eg.go(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            {
                let (lock, cvar) = &*active;
                let mut n = lock.lock().expect("worker pool poisoned");
                *n -= 1;
                cvar.notify_one();
            }
            match result {
                Ok(r) => r,
                Err(_) => Err("worker panicked".into()),
            }
        });
    }
}
