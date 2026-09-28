// Copyright 2026 AsterSQL.
//! 本 crate 内对 PD / TiKV / etcd / backuppb / kvproto 边界的本地替身。
//! 仅提供 streamhelper 推进与测试所需的最小字段与方法，不是完整协议实现。
//! 注释中的“桩”表示占位能力：真实 gRPC / etcd 客户端不在此文件接入。
//! 字段命名刻意保留 Go/protobuf 风格大写，降低与上游类型对照成本。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

/// 半开键区间替身，对应 kvproto / metapb 侧 KeyRange 用法。
/// 供 collector / flush 事件在无 protobuf 依赖时传递区间。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyRange {
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
}

/// 简易 KV 条目，供 etcd 前缀扫描等测试路径使用。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub Key: Vec<u8>,
    pub Value: Vec<u8>,
}

/// 可取消的 etcd watch 上下文；克隆共享同一取消状态。
#[derive(Clone, Default)]
pub struct WatchContext {
    canceled: Arc<AtomicBool>,
}

impl WatchContext {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
    }
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
}

/// etcd watch 事件类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchEventType {
    Put,
    Delete,
    Progress,
}

/// 带修改 revision 的 etcd watch 事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchEvent {
    pub Type: WatchEventType,
    pub Key: Vec<u8>,
    pub Value: Vec<u8>,
    pub ModRevision: i64,
}

/// 单键读取结果及线性化读取时观察到的 revision。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RevisionedValue {
    pub Value: Option<Vec<u8>>,
    pub ModRevision: i64,
    pub Revision: i64,
}

/// Region epoch 替身：Version / ConfVer，用于检查点请求身份。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionEpoch {
    pub Version: u64,
    pub ConfVer: u64,
}

/// Region 元数据替身；字段命名保持与 Go/protobuf 侧一致（大写导出）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub Id: u64,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    pub RegionEpoch: RegionEpoch,
}

impl Region {
    /// 对应 protobuf getter `GetId`。
    pub fn GetId(&self) -> u64 {
        self.Id
    }
    /// 对应 protobuf getter `GetEndKey`。
    pub fn GetEndKey(&self) -> &[u8] {
        &self.EndKey
    }
    /// 对应 protobuf getter `GetRegionEpoch`。
    pub fn GetRegionEpoch(&self) -> &RegionEpoch {
        &self.RegionEpoch
    }
}

/// Peer 替身：仅保留定位 Leader store 所需的 Id / StoreId。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Peer {
    pub Id: u64,
    pub StoreId: u64,
}

impl Peer {
    /// 对应 protobuf getter `GetStoreId`。
    pub fn GetStoreId(&self) -> u64 {
        self.StoreId
    }
}

/// 外部存储 URI 替身；可 JSON 序列化以便写入任务元数据。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StorageBackend {
    pub Uri: String,
}

/// 日志备份任务信息替身，对齐 backuppb `StreamBackupTaskInfo` 常用字段。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StreamBackupTaskInfo {
    pub Name: String,
    pub StartTs: u64,
    pub EndTs: u64,
    pub TableFilter: Vec<String>,
    pub Storage: Option<StorageBackend>,
}

/// 任务级错误记录替身，存 etcd 时用 JSON 编解码。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StreamBackupError {
    pub ErrorCode: String,
    pub ErrorMessage: String,
}

impl StreamBackupError {
    /// JSON 编码；失败映射为 String，供 MetaDataClient 路径使用。
    pub fn Marshal(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }
    /// JSON 解码；与 `Marshal` 对称。
    pub fn Unmarshal(data: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(data).map_err(|e| e.to_string())
    }
}

/// Region 身份：Id + EpochVersion，用于 flush TS 批量查询。
#[derive(Clone, Debug, Default)]
pub struct RegionIdentity {
    pub Id: u64,
    pub EpochVersion: u64,
}

/// `GetLastFlushTSOfRegion` 请求体替身。
#[derive(Clone, Debug, Default)]
pub struct GetLastFlushTSOfRegionRequest {
    pub Regions: Vec<RegionIdentity>,
}

impl GetLastFlushTSOfRegionRequest {
    /// 对应 protobuf getter `GetRegions`。
    pub fn GetRegions(&self) -> &[RegionIdentity] {
        &self.Regions
    }
}

/// Region 级错误标志替身：仅覆盖推进器关心的 EpochNotMatch / NotLeader。
#[derive(Clone, Debug, Default)]
pub struct RegionError {
    pub EpochNotMatch: bool,
    pub NotLeader: bool,
}

/// 单 Region 检查点结果：成功时带 Checkpoint，失败时带 Err。
#[derive(Clone, Debug, Default)]
pub struct RegionCheckpoint {
    pub Region: RegionIdentity,
    pub Checkpoint: u64,
    pub Err: Option<RegionError>,
}

/// 批量检查点响应替身。
#[derive(Clone, Debug, Default)]
pub struct GetLastFlushTSOfRegionResponse {
    pub Checkpoints: Vec<RegionCheckpoint>,
}

/// TiKV flush 订阅流中的单条事件。键在该本地边界中已解码。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlushEvent {
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    pub Checkpoint: u64,
}

/// 单 store 上的日志备份客户端能力；真实实现为 TiKV gRPC，此处仅为 trait 边界。
/// 测试夹具可返回固定检查点或注入 RegionError。
pub trait LogBackupClient: Send + Sync {
    fn GetLastFlushTSOfRegion(
        &self,
        req: &GetLastFlushTSOfRegionRequest,
    ) -> Result<GetLastFlushTSOfRegionResponse, String>;

    /// 建立 flush 事件流；旧客户端默认返回 Unimplemented，供能力探测。
    fn SubscribeFlushEvents(&self) -> Result<mpsc::Receiver<Vec<FlushEvent>>, String> {
        Err("Unimplemented: flush subscription".into())
    }
}

/// 按 store 获取/清理 LogBackup 客户端缓存；对应 Go 侧连接池抽象。
/// fake cluster 通过实现本 trait 注入失败与 ClearCache 钩子。
pub trait LogBackupService: Send + Sync {
    fn GetLogBackupClient(
        &self,
        storeID: u64,
    ) -> Result<std::sync::Arc<dyn LogBackupClient>, String>;
    /// 清除指定 store 的客户端缓存（空闲超时重连前会调用）。
    fn ClearCache(&self, storeID: u64) -> Result<(), String>;
}

/// MetaDataClient / AdvancerExt 使用的最小 etcd KV 面；非完整 etcd API。
/// 不含 watch / lease / txn；需要那些能力时应接真实客户端而非本桩。
pub trait EtcdKV: Send + Sync {
    fn Put(&self, key: &str, value: &[u8]) -> Result<(), String>;
    /// 写入任意字节键；Go string 可承载二进制，range start key 依赖此契约。
    fn PutBytes(&self, key: &[u8], value: &[u8]) -> Result<(), String> {
        let key = std::str::from_utf8(key).map_err(|err| err.to_string())?;
        self.Put(key, value)
    }
    fn Get(&self, key: &str) -> Result<Vec<u8>, String>;
    fn Delete(&self, key: &str) -> Result<(), String>;
    fn GetPrefix(&self, prefix: &str) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String>;
    fn DeletePrefix(&self, prefix: &str) -> Result<(), String>;
    fn GetWithRevision(&self, key: &str) -> Result<RevisionedValue, String>;
    fn GetPrefixWithRevision(&self, prefix: &str)
    -> Result<(Vec<(Vec<u8>, Vec<u8>)>, i64), String>;
    fn WatchPrefix(
        &self,
        prefix: &str,
        revision: i64,
    ) -> Result<mpsc::Receiver<WatchEvent>, String>;
    fn RequestWatchProgress(&self) -> Result<(), String>;
}

/// 进程内 HashMap 实现的 etcd 替身，供单测与 slim 集成路径使用。
#[derive(Default, Clone)]
pub struct MemEtcd {
    inner: Arc<Mutex<MemEtcdState>>,
}

#[derive(Default)]
struct MemEtcdState {
    values: HashMap<Vec<u8>, (Vec<u8>, i64)>,
    revision: i64,
    history: Vec<WatchEvent>,
    watchers: Vec<(Vec<u8>, i64, mpsc::Sender<WatchEvent>)>,
}

impl MemEtcd {
    /// 构造空的内存 etcd。
    pub fn new() -> Self {
        Self::default()
    }
}

impl EtcdKV for MemEtcd {
    fn Put(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.PutBytes(key.as_bytes(), value)
    }
    fn PutBytes(&self, key: &[u8], value: &[u8]) -> Result<(), String> {
        let mut state = self.inner.lock().unwrap();
        state.revision += 1;
        let revision = state.revision;
        let key = key.to_vec();
        state.values.insert(key.clone(), (value.to_vec(), revision));
        publish(
            &mut state,
            WatchEvent {
                Type: WatchEventType::Put,
                Key: key,
                Value: value.to_vec(),
                ModRevision: revision,
            },
        );
        Ok(())
    }
    /// 缺失键返回空 Vec，与部分 Go 调用方“读不到则空”习惯对齐。
    fn Get(&self, key: &str) -> Result<Vec<u8>, String> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .values
            .get(key.as_bytes())
            .map(|(value, _)| value.clone())
            .unwrap_or_default())
    }
    fn Delete(&self, key: &str) -> Result<(), String> {
        let mut state = self.inner.lock().unwrap();
        if state.values.remove(key.as_bytes()).is_some() {
            state.revision += 1;
            let revision = state.revision;
            publish(
                &mut state,
                WatchEvent {
                    Type: WatchEventType::Delete,
                    Key: key.as_bytes().to_vec(),
                    Value: Vec::new(),
                    ModRevision: revision,
                },
            );
        }
        Ok(())
    }
    /// 前缀扫描结果按键排序，保证测试确定性。
    fn GetPrefix(&self, prefix: &str) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        let p = prefix.as_bytes();
        let mut out = Vec::new();
        for (k, (v, _)) in self.inner.lock().unwrap().values.iter() {
            if k.starts_with(p) {
                out.push((k.clone(), v.clone()));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
    fn DeletePrefix(&self, prefix: &str) -> Result<(), String> {
        let p = prefix.as_bytes();
        let mut state = self.inner.lock().unwrap();
        let keys: Vec<_> = state
            .values
            .keys()
            .filter(|key| key.starts_with(p))
            .cloned()
            .collect();
        for key in keys {
            state.values.remove(&key);
            state.revision += 1;
            let revision = state.revision;
            publish(
                &mut state,
                WatchEvent {
                    Type: WatchEventType::Delete,
                    Key: key,
                    Value: Vec::new(),
                    ModRevision: revision,
                },
            );
        }
        Ok(())
    }
    fn GetWithRevision(&self, key: &str) -> Result<RevisionedValue, String> {
        let state = self.inner.lock().unwrap();
        let (value, mod_revision) = state
            .values
            .get(key.as_bytes())
            .map(|(value, revision)| (Some(value.clone()), *revision))
            .unwrap_or((None, 0));
        Ok(RevisionedValue {
            Value: value,
            ModRevision: mod_revision,
            Revision: state.revision,
        })
    }
    fn GetPrefixWithRevision(
        &self,
        prefix: &str,
    ) -> Result<(Vec<(Vec<u8>, Vec<u8>)>, i64), String> {
        let state = self.inner.lock().unwrap();
        let p = prefix.as_bytes();
        let mut values: Vec<_> = state
            .values
            .iter()
            .filter(|(key, _)| key.starts_with(p))
            .map(|(key, (value, _))| (key.clone(), value.clone()))
            .collect();
        values.sort_by(|a, b| a.0.cmp(&b.0));
        Ok((values, state.revision))
    }
    fn WatchPrefix(
        &self,
        prefix: &str,
        revision: i64,
    ) -> Result<mpsc::Receiver<WatchEvent>, String> {
        let (sender, receiver) = mpsc::channel();
        let mut state = self.inner.lock().unwrap();
        let prefix = prefix.as_bytes().to_vec();
        for event in &state.history {
            if event.ModRevision >= revision && event.Key.starts_with(&prefix) {
                sender.send(event.clone()).map_err(|err| err.to_string())?;
            }
        }
        state.watchers.push((prefix, revision, sender));
        Ok(receiver)
    }
    fn RequestWatchProgress(&self) -> Result<(), String> {
        let mut state = self.inner.lock().unwrap();
        let revision = state.revision;
        state.watchers.retain(|(_, _, sender)| {
            sender
                .send(WatchEvent {
                    Type: WatchEventType::Progress,
                    Key: Vec::new(),
                    Value: Vec::new(),
                    ModRevision: revision,
                })
                .is_ok()
        });
        Ok(())
    }
}

fn publish(state: &mut MemEtcdState, event: WatchEvent) {
    state.history.push(event.clone());
    state.watchers.retain(|(prefix, revision, sender)| {
        if event.ModRevision < *revision || !event.Key.starts_with(prefix) {
            return true;
        }
        sender.send(event.clone()).is_ok()
    });
}

/// 键的 next（追加 0x00），对齐 `kv.Key.Next()`，供 `locateKeyOfRegion` 使用。
pub fn key_next(key: &[u8]) -> Vec<u8> {
    let mut n = key.to_vec();
    n.push(0);
    n
}
