// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `configlist` 模块单元测试。
//
// 覆盖 FIFO 推入/弹出、阻塞等待、上下文取消、按 ID 查询删除，以及前后重排。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::{Config, Context, ContextError, TikvImporter, new_config_list};

/// 构造仅设置 `sorted_kv_dir` 的最小配置，便于用目录名区分任务。
fn sorted_cfg(dir: &str) -> Config {
    let mut cfg = crate::new_config();
    cfg.tikv_importer = TikvImporter {
        sorted_kv_dir: dir.into(),
        ..cfg.tikv_importer
    };
    cfg
}

// test_normal_push_pop 对应 Go 的 TestNormalPushPop。
/// 验证立即弹出、以及空队列时阻塞直到异步 push。
#[test]
fn test_normal_push_pop() {
    let cl = new_config_list();
    cl.push(Arc::new(Mutex::new(sorted_cfg("/tmp/sorted1"))));
    cl.push(Arc::new(Mutex::new(sorted_cfg("/tmp/sorted2"))));

    let mut start_time = Instant::now();
    let cfg = cl.pop(&Context::default()).expect("pop 1");
    assert!(start_time.elapsed() < Duration::from_millis(100));
    assert_eq!(
        "/tmp/sorted1",
        cfg.lock().unwrap().tikv_importer.sorted_kv_dir
    );

    start_time = Instant::now();
    let cfg = cl.pop(&Context::default()).expect("pop 2");
    assert!(start_time.elapsed() < Duration::from_millis(100));
    assert_eq!(
        "/tmp/sorted2",
        cfg.lock().unwrap().tikv_importer.sorted_kv_dir
    );

    // 空队列 pop 应阻塞，直到后台线程 push 第三个任务
    let start_time = Instant::now();
    let cl2 = Arc::new(cl);
    let pusher = {
        let cl2 = Arc::clone(&cl2);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(400));
            cl2.push(Arc::new(Mutex::new(sorted_cfg("/tmp/sorted3"))));
        })
    };
    let cfg = cl2.pop(&Context::default()).expect("pop 3");
    assert!(start_time.elapsed() >= Duration::from_millis(400));
    assert_eq!(
        "/tmp/sorted3",
        cfg.lock().unwrap().tikv_importer.sorted_kv_dir
    );
    pusher.join().unwrap();
}

// test_context_cancel 对应 Go 的 TestContextCancel。
/// 验证空队列 pop 在上下文取消后返回 `Cancelled`。
#[test]
fn test_context_cancel() {
    let ctx = Context::default();
    let cl = Arc::new(new_config_list());
    let cancel_ctx = ctx.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(400));
        cancel_ctx.cancel();
    });

    let start_time = Instant::now();
    let err = cl.pop(&ctx).expect_err("should cancel");
    assert!(start_time.elapsed() >= Duration::from_millis(400));
    assert_eq!(ContextError::Cancelled, err);
}

// test_get_remove 对应 Go 的 TestGetRemove。
/// 验证按 ID get/remove，以及中间删除后 pop 仍保持 FIFO 剩余顺序。
#[test]
fn test_get_remove() {
    let cl = new_config_list();
    let cfg1 = Arc::new(Mutex::new(sorted_cfg("/tmp/sorted1")));
    cl.push(Arc::clone(&cfg1));
    let cfg2 = Arc::new(Mutex::new(sorted_cfg("/tmp/sorted2")));
    cl.push(Arc::clone(&cfg2));
    let cfg3 = Arc::new(Mutex::new(sorted_cfg("/tmp/sorted3")));
    cl.push(Arc::clone(&cfg3));

    let id2 = cfg2.lock().unwrap().task_id;
    let id3 = cfg3.lock().unwrap().task_id;
    let got = cl.get(id2).expect("get cfg2");
    assert!(Arc::ptr_eq(&got, &cfg2));
    assert!(cl.get(id3 + 1000).is_none());

    assert!(cl.remove(id2));
    assert!(!cl.remove(id3 + 1000));
    assert!(cl.get(id2).is_none());

    let cfg = cl.pop(&Context::default()).unwrap();
    assert!(Arc::ptr_eq(&cfg, &cfg1));
    let cfg = cl.pop(&Context::default()).unwrap();
    assert!(Arc::ptr_eq(&cfg, &cfg3));
}

// test_move_front_back 对应 Go 的 TestMoveFrontBack。
/// 验证 move_to_front / move_to_back 调整顺序，以及对不存在 ID 返回 false。
#[test]
fn test_move_front_back() {
    let cl = new_config_list();
    let cfg1 = Arc::new(Mutex::new(sorted_cfg("/tmp/sorted1")));
    cl.push(Arc::clone(&cfg1));
    let cfg2 = Arc::new(Mutex::new(sorted_cfg("/tmp/sorted2")));
    cl.push(Arc::clone(&cfg2));
    let cfg3 = Arc::new(Mutex::new(sorted_cfg("/tmp/sorted3")));
    cl.push(Arc::clone(&cfg3));

    let id1 = cfg1.lock().unwrap().task_id;
    let id2 = cfg2.lock().unwrap().task_id;
    let id3 = cfg3.lock().unwrap().task_id;
    assert_eq!(vec![id1, id2, id3], cl.all_ids());

    assert!(cl.move_to_front(id2));
    assert_eq!(vec![id2, id1, id3], cl.all_ids());
    assert!(cl.move_to_front(id2));
    assert_eq!(vec![id2, id1, id3], cl.all_ids());
    assert!(!cl.move_to_front(123456));
    assert_eq!(vec![id2, id1, id3], cl.all_ids());

    assert!(cl.move_to_back(id2));
    assert_eq!(vec![id1, id3, id2], cl.all_ids());
    assert!(cl.move_to_back(id2));
    assert_eq!(vec![id1, id3, id2], cl.all_ids());
    assert!(!cl.move_to_back(123456));
    assert_eq!(vec![id1, id3, id2], cl.all_ids());
}
