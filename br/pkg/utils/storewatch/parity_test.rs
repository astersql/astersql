// Copyright 2026 AsterSQL.
//! storewatch 与 Go 公开契约的对照测试：回调序列与 retain 语义。
//! 用 FakeMeta 驱动 Step，断言 new/disconnect/reboot 事件顺序与文案。
//! 不依赖真实 PD；覆盖「消失再出现」时再次 OnNewStoreRegistered。
//! 事件字符串用 `new:`/`disc:`/`reboot:` 前缀，便于单缓冲聚合断言。

use std::sync::{Arc, Mutex};

use crate::{
    MakeCallback, New, Store, StoreMeta, StoreState, WithOnDisconnect, WithOnNewStoreRegistered,
    WithOnReboot,
};

/// 可变 store 列表替身：每次 Step 读取当前快照。
/// 测试线程通过同一 Arc 原地改 State/时间戳，模拟集群变化。
struct FakeMeta {
    stores: Arc<Mutex<Vec<Store>>>,
}
impl StoreMeta for FakeMeta {
    fn GetAllTiKVStores(&self) -> Result<Vec<Store>, String> {
        Ok(self.stores.lock().unwrap().clone())
    }
}

/// 串联注册/掉线/重启/retain 四段场景，校验事件串至少含两次 new:1。
#[test]
fn go_rust_public_contract_matches() {
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let e1 = events.clone();
    let e2 = events.clone();
    let e3 = events.clone();
    // 三类回调写入同一事件缓冲，便于按字符串比对。
    let cb = MakeCallback(vec![
        WithOnNewStoreRegistered(move |s| e1.lock().unwrap().push(format!("new:{}", s.Id))),
        WithOnDisconnect(move |s| e2.lock().unwrap().push(format!("disc:{}", s.Id))),
        WithOnReboot(move |s| e3.lock().unwrap().push(format!("reboot:{}", s.Id))),
    ]);
    let stores = Arc::new(Mutex::new(vec![Store {
        Id: 1,
        State: StoreState::Up,
        StartTimestamp: 10,
    }]));
    let mut w = New(
        FakeMeta {
            stores: stores.clone(),
        },
        cb,
    );
    // 首次出现 → OnNewStoreRegistered。
    w.Step().unwrap();
    // 首轮事件应只有注册，顺序与 Go 一致。
    assert_eq!(events.lock().unwrap().as_slice(), &["new:1".to_string()]);

    // Up → Offline → OnDisconnect。
    stores.lock().unwrap()[0].State = StoreState::Offline;
    w.Step().unwrap();
    assert!(events.lock().unwrap().iter().any(|e| e == "disc:1"));

    // StartTimestamp 变化 → OnReboot（与 Go updateStore 判定一致）。
    stores.lock().unwrap()[0].State = StoreState::Up;
    stores.lock().unwrap()[0].StartTimestamp = 20;
    w.Step().unwrap();
    assert!(events.lock().unwrap().iter().any(|e| e == "reboot:1"));

    // retain removes missing stores
    // 清空列表后 retain 丢弃缓存；再出现同一 id 应再次触发 new。
    stores.lock().unwrap().clear();
    w.Step().unwrap();
    stores.lock().unwrap().push(Store {
        Id: 1,
        State: StoreState::Up,
        StartTimestamp: 20,
    });
    w.Step().unwrap();
    assert_eq!(
        events.lock().unwrap().as_slice(),
        &["new:1", "disc:1", "reboot:1", "new:1"]
    );
}

/// Go `updateStore` independently checks disconnect before reboot, so a single
/// Up -> Offline update with a changed timestamp must emit both in that order.
#[test]
fn disconnect_precedes_reboot_for_the_same_update() {
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let disconnect_events = Arc::clone(&events);
    let reboot_events = Arc::clone(&events);
    let cb = MakeCallback(vec![
        WithOnDisconnect(move |s| {
            disconnect_events
                .lock()
                .unwrap()
                .push(format!("disc:{}", s.Id))
        }),
        WithOnReboot(move |s| {
            reboot_events
                .lock()
                .unwrap()
                .push(format!("reboot:{}", s.Id))
        }),
    ]);
    let stores = Arc::new(Mutex::new(vec![Store {
        Id: 1,
        State: StoreState::Up,
        StartTimestamp: 10,
    }]));
    let mut watcher = New(
        FakeMeta {
            stores: Arc::clone(&stores),
        },
        cb,
    );

    watcher.Step().unwrap();
    stores.lock().unwrap()[0] = Store {
        Id: 1,
        State: StoreState::Offline,
        StartTimestamp: 20,
    };
    watcher.Step().unwrap();

    assert_eq!(events.lock().unwrap().as_slice(), &["disc:1", "reboot:1"]);
}

struct ErrorMeta;

impl StoreMeta for ErrorMeta {
    fn GetAllTiKVStores(&self) -> Result<Vec<Store>, String> {
        Err("pd unavailable".to_string())
    }
}

/// Go `Step` annotates the store-list error and returns before mutating state.
#[test]
fn step_annotates_store_list_errors() {
    let callback = MakeCallback(vec![]);
    let mut watcher = New(ErrorMeta, callback);

    assert_eq!(
        watcher.Step().unwrap_err(),
        "failed to update store list: pd unavailable"
    );
}
