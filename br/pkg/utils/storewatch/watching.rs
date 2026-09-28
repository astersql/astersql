// Copyright 2026 AsterSQL.
//! Store lifetime watcher matching `br/pkg/utils/storewatch/watching.go`.
//!
//! 感知 TiKV store 生命周期：新注册、掉线、重启；由调用方周期性 `Step` 推进。
//! PD 列表经 `StoreMeta` 抽象；本模块不做重试，重试由上层 `GetAllTiKVStoresWithRetry` 完成。
//! 状态机基于 `lastStores` 快照差分，与 Go `updateStore`/`retain` 语义对齐。
//! 典型用于流式备份感知 store 上下线，以便调整备份目标集合。

use std::collections::{HashMap, HashSet};

/// store 运行态枚举；数值与 metapb 习惯对齐（Up/Offline/Tombstone）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreState {
    Up = 0,
    Offline = 1,
    Tombstone = 2,
}

/// 监视所需的最小 store 视图：id、状态、启动时间戳。
/// 刻意裁剪完整 metapb.Store，只保留差分所需字段。
#[derive(Clone, Debug, Default)]
pub struct Store {
    pub Id: u64,
    pub State: StoreState,
    /// 进程启动时间；变化视为 reboot（与 Go StartTimestamp 比较一致）。
    pub StartTimestamp: u64,
}

impl Default for StoreState {
    fn default() -> Self {
        StoreState::Up
    }
}

impl Store {
    /// 返回 store id，命名对齐 Go getter。
    pub fn GetId(&self) -> u64 {
        self.Id
    }
    /// 返回当前运行态。
    pub fn GetState(&self) -> StoreState {
        self.State
    }
}

/// 三类生命周期钩子；实现方可只关心子集。
pub trait Callback {
    fn OnNewStoreRegistered(&self, store: &Store);
    fn OnDisconnect(&self, store: &Store);
    fn OnReboot(&self, store: &Store);
}

/// 函数式回调集合；未设置的钩子为空操作。
pub struct DynCallback {
    onNewStoreRegistered: Option<Box<dyn Fn(&Store) + Send + Sync>>,
    onDisconnect: Option<Box<dyn Fn(&Store) + Send + Sync>>,
    onReboot: Option<Box<dyn Fn(&Store) + Send + Sync>>,
}

impl DynCallback {
    /// 有钩子则调用，无钩子静默跳过。
    pub fn OnNewStoreRegistered(&self, store: &Store) {
        if let Some(f) = &self.onNewStoreRegistered {
            f(store);
        }
    }
    /// 掉线钩子；与 trait 方法同名以便委托。
    pub fn OnDisconnect(&self, store: &Store) {
        if let Some(f) = &self.onDisconnect {
            f(store);
        }
    }
    /// 重启钩子；StartTimestamp 变化时由 updateStore 触发。
    pub fn OnReboot(&self, store: &Store) {
        if let Some(f) = &self.onReboot {
            f(store);
        }
    }
}

impl Callback for DynCallback {
    fn OnNewStoreRegistered(&self, store: &Store) {
        DynCallback::OnNewStoreRegistered(self, store)
    }
    fn OnDisconnect(&self, store: &Store) {
        DynCallback::OnDisconnect(self, store)
    }
    fn OnReboot(&self, store: &Store) {
        DynCallback::OnReboot(self, store)
    }
}

/// 构造期选项：闭包一次性写入 DynCallback 字段。
pub type DynCallbackOpt = Box<dyn FnOnce(&mut DynCallback)>;

/// 注册「新 store」钩子。
pub fn WithOnNewStoreRegistered(f: impl Fn(&Store) + Send + Sync + 'static) -> DynCallbackOpt {
    Box::new(move |cb| cb.onNewStoreRegistered = Some(Box::new(f)))
}
/// 注册「掉线」钩子（Up→Offline）。
pub fn WithOnDisconnect(f: impl Fn(&Store) + Send + Sync + 'static) -> DynCallbackOpt {
    Box::new(move |cb| cb.onDisconnect = Some(Box::new(f)))
}
/// 注册「重启」钩子（StartTimestamp 变化）。
pub fn WithOnReboot(f: impl Fn(&Store) + Send + Sync + 'static) -> DynCallbackOpt {
    Box::new(move |cb| cb.onReboot = Some(Box::new(f)))
}

/// 按选项组装 DynCallback；未提供的钩子保持 None。
pub fn MakeCallback(opts: Vec<DynCallbackOpt>) -> DynCallback {
    let mut cb = DynCallback {
        onNewStoreRegistered: None,
        onDisconnect: None,
        onReboot: None,
    };
    for opt in opts {
        opt(&mut cb);
    }
    cb
}

/// 拉取当前存活 TiKV store 列表；错误以 String 上抛供 Step 注解。
pub trait StoreMeta {
    fn GetAllTiKVStores(&self) -> Result<Vec<Store>, String>;
}

/// 监视器：持 PD 元数据客户端、回调与上一轮 store 快照。
pub struct Watcher<C: Callback, M: StoreMeta> {
    cli: M,
    cb: C,
    /// 上一轮观察到的 store；用于差分与 retain。
    lastStores: HashMap<u64, Store>,
}

/// 创建空快照监视器。
pub fn New<C: Callback, M: StoreMeta>(cli: M, cb: C) -> Watcher<C, M> {
    Watcher {
        cli,
        cb,
        lastStores: HashMap::new(),
    }
}

impl<C: Callback, M: StoreMeta> Watcher<C, M> {
    /// 拉取列表、更新差分、剔除已消失 store；失败带「failed to update store list」前缀。
    pub fn Step(&mut self) -> Result<(), String> {
        let liveStores = self
            .cli
            .GetAllTiKVStores()
            .map_err(|e| format!("failed to update store list: {e}"))?;
        let mut recorded = HashSet::new();
        for store in liveStores {
            self.updateStore(store.clone());
            recorded.insert(store.GetId());
        }
        // 本轮未出现的 id 从 lastStores 删除，避免幽灵缓存。
        self.retain(&recorded);
        Ok(())
    }

    // 写入新快照并按差分触发回调：首见→注册；Up→Offline→掉线；时间戳变→重启。
    fn updateStore(&mut self, newStore: Store) {
        let id = newStore.GetId();
        let last = self.lastStores.insert(id, newStore.clone());
        match last {
            None => self.cb.OnNewStoreRegistered(&newStore),
            Some(lastStore) => {
                if lastStore.GetState() == StoreState::Up
                    && newStore.GetState() == StoreState::Offline
                {
                    self.cb.OnDisconnect(&newStore);
                }
                // 重启判定独立于状态迁移，Offline 后带新时间戳回来也会触发。
                if lastStore.StartTimestamp != newStore.StartTimestamp {
                    self.cb.OnReboot(&newStore);
                }
            }
        }
    }

    // 仅保留本轮仍存活的 store id。
    fn retain(&mut self, storeSet: &HashSet<u64>) {
        self.lastStores.retain(|id, _| storeSet.contains(id));
    }
}
