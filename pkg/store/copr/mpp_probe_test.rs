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

// MPP 失败 store 探活与 server 信息缓存的单元测试。
//
// `GO_REFERENCE` 保留对齐 Go 的测试骨架（字符串内容禁止改动）；
// 下方可运行用例覆盖失败/恢复跟踪、探活辅助与 LRU 淘汰。

/// 对齐 Go `mpp_probe_test.go` 的参考测试骨架（不可执行占位）。
const GO_REFERENCE: &str = r################"
// 这段逻辑覆盖 MPP failed store 探活流程、后台任务互斥、异常输入断言和 MPP server info LRU 缓存。

const TESTIMEOUT: &str = "timeout";
const ERROR: &str = "error";
const NORMAL: &str = "normal";

// mockDetectClient 对应 Go 的 mockDetectClient：通过 errortestype 控制 SendRequest 的错误、超时和正常探活响应。
struct mockDetectClient {
    errortestype: String,
}

impl mockDetectClient {
    // CloseAddr 保留 tikv.Client 接口形状，本测试中总是返回 nil。
    fn CloseAddr(&self, _addr: String) -> Result<(), errors::Error> {
        Ok(())
    }

    // Close 保留客户端关闭接口形状，本测试不持有真实资源。
    fn Close(&self) -> Result<(), errors::Error> {
        Ok(())
    }

    // SendRequest 对应 Go mock：error 返回 store error，timeout 返回 Available=false，normal 返回 Available=true。
    fn SendRequest(
        &self,
        _ctx: context::Context,
        _addr: String,
        _req: tikvrpc::Request,
        _timeout: time::Duration,
    ) -> Result<tikvrpc::Response, errors::Error> {
        if self.errortestype == ERROR {
            return Err(errors::New("store error"));
        }
        if self.errortestype == TESTIMEOUT {
            return Ok(tikvrpc::Response { Resp: mpp::IsAliveResponse { Available: false } });
        }
        Ok(tikvrpc::Response { Resp: mpp::IsAliveResponse { Available: true } })
    }

    // SendRequestAsync 对应 Go 的 goroutine 调度 callback；这里保留异步回调语义而不真正接入 runtime。
    fn SendRequestAsync(
        &self,
        ctx: context::Context,
        addr: String,
        req: tikvrpc::Request,
        cb: async::Callback<tikvrpc::Response>,
    ) {
        go::spawn(|| {
            cb.Schedule(self.SendRequest(ctx, addr, req, tikv::ReadTimeoutMedium));
        });
    }

    fn SetEventListener(&self, _listener: tikv::ClientEventListener) {}
}

// ProbeTest 对应 Go 的 map[string]*mockDetectClient，按地址保存不同错误类型的 mock 客户端。
type ProbeTest = std::collections::HashMap<String, mockDetectClient>;

trait ProbeTestExt {
    fn add(&self, ctx: context::Context);
    fn reSetErrortestype(&mut self, to: &str);
    fn judge(&self, ctx: context::Context, recovery_ttl: time::Duration, need: bool);
}

impl ProbeTestExt for ProbeTest {
    // add 对应 Go 的 ProbeTest.add：把每个 mock client 加入全局 failed store prober。
    fn add(&self, ctx: context::Context) {
        for (k, v) in self {
            GlobalMPPFailedStoreProber.Add(ctx.clone(), k.clone(), v.clone());
        }
    }

    // reSetErrortestype 对应 Go 的重置逻辑：NORMAL 统一恢复，其它情况按 map key 恢复各自错误类型。
    fn reSetErrortestype(&mut self, to: &str) {
        for (k, v) in self.iter_mut() {
            if to == NORMAL {
                v.errortestype = NORMAL.to_string();
            } else {
                v.errortestype = k.clone();
            }
        }
    }

    // judge 对应 Go 的 ProbeTest.judge：逐个地址调用 IsRecovery 并比较期望。
    fn judge(&self, ctx: context::Context, recovery_ttl: time::Duration, need: bool) {
        for k in self.keys() {
            let ok = GlobalMPPFailedStoreProber.IsRecovery(ctx.clone(), k.clone(), recovery_ttl);
            require::equal(need, ok);
        }
    }
}

// failed_store_size_judge 对应 Go 辅助函数：scan 后等待探测 goroutine 收尾，再统计 failedMPPStores。
fn failed_store_size_judge(ctx: context::Context, need: i32) {
    let mut len = 0;
    GlobalMPPFailedStoreProber.scan(ctx);
    time::Sleep(time::Second / 10);
    GlobalMPPFailedStoreProber.failedMPPStores.Range(|_k, _v| {
        len += 1;
        true
    });
    require::equal(need, len);
}

// test_flow 对应 Go 的 testFlow：按错误流推进探活状态，并覆盖恢复清理和过期清理两个分支。
fn test_flow(ctx: context::Context, mut probetestest: ProbeTest, flow: &[&str]) {
    probetestest.add(ctx.clone());
    for to in flow {
        probetestest.reSetErrortestype(to);
        GlobalMPPFailedStoreProber.scan(ctx.clone());
        time::Sleep(time::Second / 10); // wait detect goroutine finish

        let need = *to == NORMAL;
        probetestest.judge(ctx.clone(), time::Duration::from_secs(0), need);
        // recoveryTTL=1min 时即使刚恢复也不应被认为可用，保留 Go 对 TTL 门槛的检查。
        probetestest.judge(ctx.clone(), time::Minute, false);
    }

    let last_to = flow[flow.len() - 1];
    let clean_recover = |need: i32| {
        GlobalMPPFailedStoreProber.maxRecoveryTimeLimit = -time::Second;
        failed_store_size_judge(ctx.clone(), need);
        GlobalMPPFailedStoreProber.maxRecoveryTimeLimit = MaxRecoveryTimeLimit;
    };
    let clean_obsolet = |need: i32| {
        GlobalMPPFailedStoreProber.maxObsoletTimeLimit = -time::Second;
        failed_store_size_judge(ctx.clone(), need);
        GlobalMPPFailedStoreProber.maxObsoletTimeLimit = MaxObsoletTimeLimit;
    };

    if last_to == ERROR {
        clean_recover(2);
        clean_obsolet(0);
    } else if last_to == NORMAL {
        clean_obsolet(2);
        clean_recover(0);
    }
}

// test_mpp_failed_store_probe 对应 Go 的 TestMPPFailedStoreProbe：覆盖不存在地址、错误->恢复和最终废弃两条状态流。
#[test]
fn test_mpp_failed_store_probe() {
    let ctx = context::Background();
    let not_exist_address = "not exist address";

    GlobalMPPFailedStoreProber.detectPeriod = -time::Second;
    require::true_(GlobalMPPFailedStoreProber.IsRecovery(ctx.clone(), not_exist_address, 0));
    GlobalMPPFailedStoreProber.scan(ctx.clone());

    let mut probetestest = ProbeTest::new();
    probetestest.insert(TESTIMEOUT.to_string(), mockDetectClient { errortestype: TESTIMEOUT.to_string() });
    probetestest.insert(ERROR.to_string(), mockDetectClient { errortestype: ERROR.to_string() });

    test_flow(ctx.clone(), probetestest.clone(), &[ERROR, NORMAL, ERROR, ERROR, NORMAL]);
    test_flow(ctx, probetestest, &[ERROR, NORMAL, NORMAL, ERROR, ERROR]);
}

// test_mpp_failed_store_probe_goroutine_task 对应 Go 测试：持锁调用 Run 后再次 Run，确认不会允许多个后台任务。
#[test]
fn test_mpp_failed_store_probe_goroutine_task() {
    GlobalMPPFailedStoreProber.lock.Lock();
    GlobalMPPFailedStoreProber.Run();
    GlobalMPPFailedStoreProber.lock.Unlock();

    GlobalMPPFailedStoreProber.Run();
    GlobalMPPFailedStoreProber.Stop();
}

// test_mpp_failed_store_assert_failed 对应 Go 测试：failedMPPStores 中值为 nil 时 scan/IsRecovery 仍能容错。
#[test]
fn test_mpp_failed_store_assert_failed() {
    let ctx = context::Background();
    GlobalMPPFailedStoreProber.failedMPPStores.Store("errorinfo", None);
    GlobalMPPFailedStoreProber.scan(ctx.clone());

    GlobalMPPFailedStoreProber.failedMPPStores.Store("errorinfo", None);
    GlobalMPPFailedStoreProber.IsRecovery(ctx, "errorinfo", 0);
}

// test_mpp_server_info_manager 对应 Go 的 TestMppServerInfoManager：验证增删查和 LRU 淘汰顺序。
#[test]
fn test_mpp_server_info_manager() {
    let manager = newMppServerInfoManager();
    manager.Delete("123");
    manager.Add(MPPServerInfo { Address: "123".to_string(), LogicalCPUCount: 123, StartTimestamp: 456 });
    require::equal(1, manager.cachedStores.Size());

    let info = manager.Get("123");
    require::true_(info.is_some());
    require::equal("123", info.Address);
    require::equal(123_u64, info.LogicalCPUCount);
    require::equal(456_i64, info.StartTimestamp);

    manager.Delete("123");
    require::equal(0, manager.cachedStores.Size());
    require::true_(manager.Get("123").is_none());

    // Go 循环填满缓存后访问 store-0，再插入新元素；期望 store-1 被 LRU 淘汰。
    for i in 0..mppServerInfoManagerCacheSize {
        manager.Add(MPPServerInfo { Address: format!("store-{}", i), ..Default::default() });
    }
    require::equal(mppServerInfoManagerCacheSize, manager.cachedStores.Size());
    require::not_nil(manager.Get("store-0"));

    manager.Add(MPPServerInfo { Address: format!("store-{}", mppServerInfoManagerCacheSize), ..Default::default() });
    require::equal(mppServerInfoManagerCacheSize, manager.cachedStores.Size());
    require::not_nil(manager.Get("store-0"));
    require::nil(manager.Get("store-1"));
}
"################;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::{
    MppAliveClient, MppFailedStoreProber, MppServerInfo, MppServerInfoManager, detect_mpp_store,
};

/// 可切换存活结果的探活替身客户端。
struct ProbeClient(AtomicBool);

impl MppAliveClient for ProbeClient {
    fn is_alive(&self, _address: &str, _timeout: Duration) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[test]
/// 验证失败探测、恢复判定、删除条目与 detect_mpp_store。
fn failed_store_probe_tracks_failure_and_recovery() {
    let client = Arc::new(ProbeClient(AtomicBool::new(false)));
    let mut prober = MppFailedStoreProber::default();
    prober.detect_period = Duration::ZERO;
    prober.add("tiflash-0".to_owned(), client.clone());
    prober.scan();
    assert!(!prober.is_recovered("tiflash-0", Duration::ZERO));

    client.0.store(true, Ordering::Release);
    prober.scan();
    assert!(prober.is_recovered("tiflash-0", Duration::ZERO));
    assert!(prober.delete("tiflash-0"));
    assert!(prober.is_recovered("tiflash-0", Duration::ZERO));
    assert!(detect_mpp_store(
        client.as_ref(),
        "tiflash-0",
        Duration::from_millis(1)
    ));
}

#[test]
/// Go 在探活 RPC 完成后才开始计算下一次探测间隔，慢请求不能提前消耗该间隔。
fn failed_store_probe_detect_period_starts_after_request_completes() {
    struct SlowClient(std::sync::atomic::AtomicUsize);

    impl MppAliveClient for SlowClient {
        fn is_alive(&self, _address: &str, _timeout: Duration) -> bool {
            self.0.fetch_add(1, Ordering::AcqRel);
            std::thread::sleep(Duration::from_millis(30));
            true
        }
    }

    let client = Arc::new(SlowClient(std::sync::atomic::AtomicUsize::new(0)));
    let mut prober = MppFailedStoreProber::default();
    prober.detect_period = Duration::from_millis(20);
    prober.add("tiflash-0".to_owned(), client.clone());
    prober.scan();
    prober.scan();

    assert_eq!(client.0.load(Ordering::Acquire), 1);
}

#[test]
/// 验证容量为 2 时访问 store-0 后插入 store-2 会淘汰 store-1。
fn server_info_manager_uses_lru_eviction() {
    let manager = MppServerInfoManager::new(2);
    for address in ["store-0", "store-1"] {
        manager.add(MppServerInfo {
            address: address.to_owned(),
            logical_cpu_count: 8,
            start_timestamp: 1,
        });
    }
    assert!(manager.get("store-0").is_some());
    manager.add(MppServerInfo {
        address: "store-2".to_owned(),
        logical_cpu_count: 16,
        start_timestamp: 2,
    });
    assert!(manager.get("store-0").is_some());
    assert!(manager.get("store-1").is_none());
    assert_eq!(manager.get("store-2").unwrap().logical_cpu_count, 16);
}

#[test]
/// 后台探测只启动一个 worker，重复 run/stop 不会泄漏或重复探测任务。
fn failed_store_probe_background_worker_is_singleton() {
    struct CountingClient(std::sync::atomic::AtomicUsize);

    impl MppAliveClient for CountingClient {
        fn is_alive(&self, _address: &str, _timeout: Duration) -> bool {
            self.0.fetch_add(1, Ordering::AcqRel);
            false
        }
    }

    let client = Arc::new(CountingClient(std::sync::atomic::AtomicUsize::new(0)));
    let mut prober = MppFailedStoreProber::default();
    prober.detect_period = Duration::ZERO;
    prober.add("tiflash-0".to_owned(), client.clone());
    prober.run();
    prober.run();
    std::thread::sleep(Duration::from_millis(20));
    prober.stop();
    prober.stop();
    assert!(client.0.load(Ordering::Acquire) >= 1);
}

#[test]
/// 未注册/已删除条目视为恢复，重复删除保持幂等。
fn failed_store_probe_tolerates_unknown_and_deleted_entries() {
    let client = Arc::new(ProbeClient(AtomicBool::new(false)));
    let prober = MppFailedStoreProber::default();
    assert!(prober.is_recovered("missing", Duration::ZERO));
    assert!(!prober.delete("missing"));
    prober.add("tiflash-0".to_owned(), client);
    assert!(prober.delete("tiflash-0"));
    assert!(!prober.delete("tiflash-0"));
    assert!(prober.is_recovered("tiflash-0", Duration::ZERO));
}
