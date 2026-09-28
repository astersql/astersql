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

// util 包安全与配套工具的迁移单元测试。
//
// 覆盖 TLS 配置构建、键空间 split、URL 解析、令牌限流、会话池、
// WaitGroup 与 WorkerPool 等与 Go 侧行为对齐的场景。
// 本文件由 `security_formal_aster_unit_test.rs` 通过 `include!` 引入。

use std::io::BufReader;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_util::{
    security, session_pool, split, tokenlimiter, urls, util, wait_group_wrapper, worker_pool,
};

/// 校验 TLS 构建器：空配置返回 None、非法 CA 报错、合法证书可建客户端/服务端配置。
#[test]
fn security_builder_validates_input_and_common_names() {
    assert!(security::NewTLSConfig(Vec::new()).unwrap().is_none());
    assert!(security::NewTLSConfig(vec![security::WithCAContent(b"not pem".to_vec())]).is_err());

    let mut params = rcgen::CertificateParams::new(vec!["server".to_owned()]).unwrap();
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "server");
    let key = rcgen::KeyPair::generate().unwrap();
    let certificate = params.self_signed(&key).unwrap();
    let ca_pem = certificate.pem().into_bytes();
    let config = security::NewTLSConfig(vec![
        security::WithCAContent(ca_pem),
        security::WithCertAndKeyContent(
            certificate.pem().into_bytes(),
            key.serialize_pem().into_bytes(),
        ),
        security::WithVerifyCommonName(vec![" server ".to_owned()]),
        security::WithMinTLSVersion(0x0304),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(config.min_tls_version(), 0x0304);
    assert!(config.verify_common_name(certificate.der()).is_ok());
    assert!(config.client_config().is_ok());
    assert!(config.server_config().is_ok());
}

/// 校验键空间 split 与 Go 大端算法一致：切分点有序且长度为 8 字节。
#[test]
fn split_matches_go_big_endian_algorithm() {
    let values = split::GetValuesList(b"a".to_vec(), b"z".to_vec(), 4, Vec::new());
    assert_eq!(values.len(), 3);
    assert!(values.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(values.iter().all(|value| value.len() == 8));
}

/// 校验主机端口 URL 解析规则与 Go 一致：支持 http(s)/裸地址，拒绝无端口与带 path。
#[test]
fn urls_match_go_validation() {
    assert_eq!(
        urls::ParseHostPortAddr(" 127.0.0.1:2379,https://localhost:2380 ").unwrap(),
        vec!["127.0.0.1:2379", "https://localhost:2380"]
    );
    assert!(urls::ParseHostPortAddr("127.0.0.1").is_err());
    assert!(urls::ParseHostPortAddr("ftp://localhost:21").is_err());
    assert!(urls::ParseHostPortAddr("https://localhost:2379/path").is_err());
}

/// 校验令牌限流器：令牌耗尽时 Get 阻塞，Put 归还后才继续获取。
#[test]
fn token_limiter_blocks_until_a_token_is_returned() {
    let limiter = tokenlimiter::NewTokenLimiter(2);
    assert_eq!(limiter.Count(), 2);
    let first = limiter.Get();
    let second = limiter.Get();
    let cloned = Arc::clone(&limiter);
    let acquired = Arc::new(AtomicBool::new(false));
    let acquired_in_thread = Arc::clone(&acquired);
    let handle = std::thread::spawn(move || {
        let token = cloned.Get();
        acquired_in_thread.store(true, Ordering::SeqCst);
        cloned.Put(token);
    });
    std::thread::sleep(Duration::from_millis(20));
    assert!(!acquired.load(Ordering::SeqCst));
    limiter.Put(first);
    handle.join().unwrap();
    assert!(acquired.load(Ordering::SeqCst));
    limiter.Put(second);
}

/// 会话池测试用资源：记录 close 次数与引用计数变化。
#[derive(Default)]
struct TestResource {
    closed: AtomicUsize,
    refs: AtomicI32,
}

impl session_pool::Resource for TestResource {
    fn close(&self) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// 校验会话池复用、溢出关闭、回调与关闭后 Put/Get 语义与 Go 一致。
#[test]
fn session_pool_reuses_closes_and_runs_callbacks_like_go() {
    let made = Arc::new(Mutex::new(Vec::<Arc<TestResource>>::new()));
    let made_by_factory = Arc::clone(&made);
    let factory: session_pool::Factory = Arc::new(move || {
        let resource = Arc::new(TestResource::default());
        made_by_factory.lock().unwrap().push(Arc::clone(&resource));
        Ok(resource)
    });
    let on_get: session_pool::ResourceCallback = Arc::new(|resource| {
        resource
            .as_any()
            .downcast_ref::<TestResource>()
            .unwrap()
            .refs
            .fetch_add(1, Ordering::SeqCst);
    });
    let on_put: session_pool::ResourceCallback = Arc::new(|resource| {
        resource
            .as_any()
            .downcast_ref::<TestResource>()
            .unwrap()
            .refs
            .fetch_sub(1, Ordering::SeqCst);
    });
    let pool = session_pool::NewSessionPool(1, factory, Some(on_get), Some(on_put), None);
    let first = pool.Get().unwrap();
    let second = pool.Get().unwrap();
    pool.Put(first);
    pool.Put(second);
    let resources = made.lock().unwrap();
    assert_eq!(resources[0].closed.load(Ordering::SeqCst), 0);
    assert_eq!(resources[1].closed.load(Ordering::SeqCst), 1);
    drop(resources);
    pool.Close();
    pool.Close();
    assert_eq!(made.lock().unwrap()[0].closed.load(Ordering::SeqCst), 1);
    let returned_after_close = Arc::new(TestResource::default());
    pool.Put(returned_after_close.clone());
    assert_eq!(returned_after_close.closed.load(Ordering::SeqCst), 1);
    assert_eq!(returned_after_close.refs.load(Ordering::SeqCst), -1);
    assert_eq!(pool.Get().err().unwrap().to_string(), "session pool closed");
}

/// 校验字节换算、字符串解析、标识符检查、读行与同集群判断等工具边界行为。
#[test]
fn utility_functions_preserve_go_edges() {
    assert_eq!(util::ByteToGiB(1_073_741_824.0), 1.0);
    assert_eq!(
        util::Str2Int64Map("1,bad,2"),
        [0, 1, 2].into_iter().collect()
    );
    assert_eq!(
        util::FmtNonASCIIPrintableCharToHex("a\u{7f}\u{80}", 8, false),
        "a\\xC2\\x80"
    );
    assert!(util::IsInCorrectIdentifierName(""));
    assert!(util::IsInCorrectIdentifierName("name "));

    let mut reader = BufReader::new("first\r\nsecond\n".as_bytes());
    assert_eq!(
        util::ReadLines(&mut reader, 2, 16).unwrap(),
        vec![b"first".to_vec(), b"second".to_vec()]
    );

    let (same, first, second) = util::CheckIfSameCluster(
        (),
        |_| Ok(vec!["pd-1".to_owned(), "pd-2".to_owned()]),
        |_| Ok(vec!["pd-3".to_owned(), "pd-2".to_owned()]),
    )
    .unwrap();
    assert!(same);
    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 2);
}

/// 校验 WaitGroupWrapper：并发任务完成计数，以及 panic 恢复回调。
#[test]
fn wait_group_wrappers_finish_and_recover_panics() {
    let wrapper = wait_group_wrapper::WaitGroupWrapper::default();
    let completed = Arc::new(AtomicUsize::new(0));
    for _ in 0..4 {
        let completed = Arc::clone(&completed);
        wrapper.Run(move || {
            completed.fetch_add(1, Ordering::SeqCst);
        });
    }
    wrapper.Wait();
    assert_eq!(completed.load(Ordering::SeqCst), 4);

    let recovered = Arc::new(AtomicBool::new(false));
    let recovered_by_callback = Arc::clone(&recovered);
    wrapper.RunWithRecover(
        || panic!("expected"),
        Some(move |payload: Option<wait_group_wrapper::PanicPayload>| {
            assert!(payload.is_some());
            recovered_by_callback.store(true, Ordering::SeqCst);
        }),
    );
    wrapper.Wait();
    assert!(recovered.load(Ordering::SeqCst));
}

/// 校验 WorkerPool：容量上限、回收空闲 worker、以及带 ID 的 Apply。
#[test]
fn worker_pool_limits_and_recycles_workers() {
    let pool = worker_pool::NewWorkerPool(2, "test".to_owned());
    assert_eq!(pool.Limit(), 2);
    let first = pool.ApplyWorker();
    let second = pool.ApplyWorker();
    assert_ne!(first.ID, second.ID);
    assert!(!pool.HasWorker());
    pool.RecycleWorker(first);
    pool.RecycleWorker(second);
    assert_eq!(pool.IdleCount(), 2);

    let (sender, receiver) = std::sync::mpsc::channel();
    pool.ApplyWithID(move |id| sender.send(id).unwrap());
    let id = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!((1..=2).contains(&id));
    for _ in 0..100 {
        if pool.IdleCount() == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(pool.IdleCount(), 2);
}

/// 校验增强 WaitGroup 与 ErrorGroup 的生命周期：正常完成与 panic 错误传播。
#[test]
fn enhanced_wait_group_and_error_group_match_go_lifecycle() {
    let enhanced = wait_group_wrapper::NewWaitGroupEnhancedWrapper("test".to_owned(), None, false);
    let completed = Arc::new(AtomicBool::new(false));
    let completed_in_thread = Arc::clone(&completed);
    enhanced.Run(
        move || completed_in_thread.store(true, Ordering::SeqCst),
        "worker".to_owned(),
    );
    enhanced.Wait();
    assert!(completed.load(Ordering::SeqCst));
    assert!(!enhanced.check());

    let group = wait_group_wrapper::NewErrorGroupWithRecover();
    group.Go(|| panic!("group panic"));
    assert!(
        group
            .Wait()
            .unwrap_err()
            .to_string()
            .contains("group panic")
    );
}
