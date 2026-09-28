// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 会话池（session pool）单元测试：容量、回调与关闭后行为。
//
// 会话池复用已建立的会话/资源，避免反复创建销毁。本文件用桩资源
// `testResource` 验证 Get/Put 引用计数、超容量销毁，以及 Close 后 Put/Get。

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::any::Any;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::session_pool::{Factory, NewSessionPool, PooledResource, Resource, ResourceCallback};

/// 测试用池资源：`status` 表示是否已 close，`refCount` 跟踪回调增减。
#[derive(Default)]
struct testResource {
    /// 关闭标记：`close` 写入 1，供断言资源是否被销毁。
    status: AtomicIsize,
    /// 引用计数：on_get 加一，on_put/on_destroy 减一。
    refCount: AtomicIsize,
}

impl Resource for testResource {
    /// 池销毁资源时调用；将 `status` 置为 1。
    fn close(&self) {
        self.status.store(1, Ordering::SeqCst);
    }

    /// 下转型入口，供测试从 `PooledResource` 取回 `testResource`。
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 将池返回的 `PooledResource` 下转为 `testResource`；类型不符则 panic。
fn as_test_resource(resource: &PooledResource) -> &testResource {
    resource
        .as_any()
        .downcast_ref::<testResource>()
        .expect("resource should be *testResource in this test")
}

/// 覆盖容量为 1 时的借还、溢出关闭、重复 Close 与关闭后错误路径。
#[test]
fn TestSessionPool() {
    // made 记录工厂创建过的全部资源，便于 Put 后检查首个资源的 refCount。
    let made = Arc::new(Mutex::new(Vec::<Arc<testResource>>::new()));
    let made_by_factory = Arc::clone(&made);
    let factory: Factory = Arc::new(move || {
        let resource = Arc::new(testResource::default());
        made_by_factory
            .lock()
            .expect("made mutex")
            .push(Arc::clone(&resource));
        Ok(resource as PooledResource)
    });

    // on_get / on_put / on_destroy 分别模拟借出、归还空闲槽、销毁时的引用计数调整。
    let on_get: ResourceCallback = Arc::new(|r: &PooledResource| {
        as_test_resource(r).refCount.fetch_add(1, Ordering::SeqCst);
    });
    let on_put: ResourceCallback = Arc::new(|r: &PooledResource| {
        as_test_resource(r).refCount.fetch_sub(1, Ordering::SeqCst);
    });
    let on_destroy: ResourceCallback = Arc::new(|r: &PooledResource| {
        as_test_resource(r).refCount.fetch_sub(1, Ordering::SeqCst);
    });

    // 容量 1：第二个 Put 时若空闲槽已满，应走 destroy 而非放回池中。
    let pool = NewSessionPool(1, factory, Some(on_get), Some(on_put), Some(on_destroy));

    let tr = pool.Get().expect("first Get should create a resource");
    assert_eq!(1, as_test_resource(&tr).refCount.load(Ordering::SeqCst));

    let tr1 = pool.Get().expect("second Get should create a resource");
    assert_eq!(1, as_test_resource(&tr1).refCount.load(Ordering::SeqCst));

    pool.Put(tr);
    let resources = made.lock().expect("made mutex");
    assert_eq!(0, resources[0].refCount.load(Ordering::SeqCst));
    drop(resources);

    // Capacity is 1, so tr1 is closed.
    // 空闲槽已满：Put(tr1) 触发 on_destroy 并 close，status 变为 1。
    pool.Put(Arc::clone(&tr1));
    assert_eq!(0, as_test_resource(&tr1).refCount.load(Ordering::SeqCst));
    assert_eq!(1, as_test_resource(&tr1).status.load(Ordering::SeqCst));

    // 重复 Close 应幂等；关闭后再 Put 仍会走 destroy 回调（refCount 再减）。
    pool.Close();
    pool.Close();

    pool.Put(Arc::clone(&tr1));
    assert_eq!(-1, as_test_resource(&tr1).refCount.load(Ordering::SeqCst));

    // 关闭后 Get 应失败，错误文案与 Go 侧 “session pool closed” 对齐。
    let result = pool.Get();
    assert!(result.is_err());
    assert_eq!("session pool closed", result.err().unwrap().to_string());
}
