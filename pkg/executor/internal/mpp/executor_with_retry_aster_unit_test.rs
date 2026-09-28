// Copyright 2026 AsterSQL.

// `ExecutorWithRetry` 与 `MppCoordinatorManager` 的单元测试。
//
// 用脚本化 TestCoordinator 验证：注册/注销、缓冲 FIFO、Close 幂等，
// 以及可恢复错误时重建 gather、丢弃旧缓冲并继续产出新结果。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_util_memory as memory;

use crate::executor_with_retry::{
    CoordinatorFactory, CoordinatorRegistry, CoordinatorUniqueId, MppCoordinatorManager,
    MppRecoveryConfig, NewExecutorWithRetry, SharedMppCoordinator,
};
use crate::recovery_handler::{HandlerImpl, RecoveryInfo};

/// 最小 ResultSubset：仅携带字节数据。
struct TestSubset(Vec<u8>);

impl kv::ResultSubset for TestSubset {
    fn GetData(&self) -> &[u8] {
        &self.0
    }
    fn GetStartKey(&self) -> kv::Key {
        kv::Key::default()
    }
    fn MemSize(&self) -> i64 {
        self.0.len() as i64
    }
    fn RespTime(&self) -> Duration {
        Duration::ZERO
    }
}

/// 协调器 Next 脚本步骤：数据 / 错误 / 流结束。
enum Step {
    Data(Vec<u8>),
    Error(&'static str),
    End,
}

/// 按步骤队列吐出响应的测试协调器。
struct TestCoordinator {
    steps: VecDeque<Step>,
    closes: Arc<AtomicUsize>,
}

/// 空操作 StatusReporter。
struct TestStatusReporter;

impl kv::MppStatusReporter for TestStatusReporter {
    fn ReportStatus(&self, _: kv::ReportStatusRequest) -> Result<(), errors::SharedError> {
        Ok(())
    }
}

impl kv::Response for TestCoordinator {
    fn Next(
        &mut self,
        _: &kv::Context,
    ) -> Result<Option<Box<dyn kv::ResultSubset>>, errors::SharedError> {
        match self.steps.pop_front().unwrap_or(Step::End) {
            Step::Data(data) => Ok(Some(Box::new(TestSubset(data)))),
            Step::Error(message) => Err(errors::New(message)),
            Step::End => Ok(None),
        }
    }
    fn Close(&mut self) -> Result<(), errors::SharedError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl kv::MppCoordinator for TestCoordinator {
    fn Execute(&mut self, _: &kv::Context) -> Result<Vec<kv::KeyRange>, errors::SharedError> {
        Ok(vec![kv::KeyRange::default()])
    }
    fn ReportStatus(&mut self, _: kv::ReportStatusRequest) -> Result<(), errors::SharedError> {
        Ok(())
    }
    fn StatusReporter(&self) -> Arc<dyn kv::MppStatusReporter> {
        Arc::new(TestStatusReporter)
    }
    fn IsClosed(&self) -> bool {
        false
    }
    fn GetNodeCnt(&self) -> i32 {
        2
    }
}

/// 每次 Build 弹出一段脚本步骤的 Factory。
struct TestFactory {
    scripts: Mutex<VecDeque<VecDeque<Step>>>,
    builds: AtomicUsize,
    closes: Arc<AtomicUsize>,
}

impl CoordinatorFactory for TestFactory {
    fn Build(&self, _: u64) -> Result<Box<dyn kv::MppCoordinator>, errors::SharedError> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(TestCoordinator {
            steps: self
                .scripts
                .lock()
                .expect("script lock")
                .pop_front()
                .expect("script"),
            closes: self.closes.clone(),
        }))
    }
}

/// 记录 Register/Unregister 调用的测试注册表。
#[derive(Default)]
struct TestRegistry {
    registered: Mutex<Vec<CoordinatorUniqueId>>,
    unregistered: Mutex<Vec<CoordinatorUniqueId>>,
    coordinators: Mutex<Vec<SharedMppCoordinator>>,
}

impl CoordinatorRegistry for TestRegistry {
    fn Register(
        &self,
        id: CoordinatorUniqueId,
        coordinator: SharedMppCoordinator,
        _: Arc<dyn kv::MppStatusReporter>,
    ) -> Result<(), errors::SharedError> {
        self.registered.lock().expect("registered lock").push(id);
        self.coordinators
            .lock()
            .expect("coordinator lock")
            .push(coordinator);
        Ok(())
    }
    fn Unregister(&self, id: CoordinatorUniqueId) {
        self.unregistered
            .lock()
            .expect("unregistered lock")
            .push(id);
    }
}

/// 始终接受恢复的 HandlerImpl。
struct AcceptRecovery;
impl HandlerImpl for AcceptRecovery {
    fn chooseHandlerImpl(&self, _: &errors::SharedError) -> bool {
        true
    }
    fn doRecovery(&self, _: &RecoveryInfo) -> Result<(), errors::SharedError> {
        Ok(())
    }
}

/// 将步骤列表转为队列。
fn script(steps: Vec<Step>) -> VecDeque<Step> {
    steps.into()
}

/// Manager：注册成功、重复注册失败、Unregister 后 Len=0。
#[test]
fn concrete_manager_registers_routes_and_unregisters_shared_coordinator() {
    let manager = MppCoordinatorManager::default();
    let id = CoordinatorUniqueId {
        query_id: kv::MPPQueryID::default(),
        gather_id: 7,
    };
    let coordinator: SharedMppCoordinator = Arc::new(Mutex::new(Box::new(TestCoordinator {
        steps: VecDeque::new(),
        closes: Arc::new(AtomicUsize::new(0)),
    })));
    manager
        .Register(id, coordinator.clone(), Arc::new(TestStatusReporter))
        .expect("register");
    assert_eq!(manager.Len(), 1);
    assert!(
        manager
            .Register(id, coordinator, Arc::new(TestStatusReporter))
            .is_err()
    );
    manager.Unregister(id);
    assert_eq!(manager.Len(), 0);
}

/// 缓冲响应按 FIFO 弹出；Close 幂等且只注销一次。
#[test]
fn held_responses_remain_fifo_and_close_unregisters_once() {
    let closes = Arc::new(AtomicUsize::new(0));
    let factory = Arc::new(TestFactory {
        scripts: Mutex::new(
            vec![script(vec![
                Step::Data(vec![1]),
                Step::Data(vec![2]),
                Step::End,
            ])]
            .into(),
        ),
        builds: AtomicUsize::new(0),
        closes: closes.clone(),
    });
    let registry = Arc::new(TestRegistry::default());
    let mut parent = memory::tracker::NewTracker(9, 0);
    let mut executor = NewExecutorWithRetry(
        kv::Context::todo(),
        &mut parent,
        kv::MPPQueryID::default(),
        Arc::new(AtomicU64::new(0)),
        factory,
        registry.clone(),
        MppRecoveryConfig {
            enabled: true,
            holder_capacity: 2,
            ..Default::default()
        },
    )
    .expect("retry executor");

    assert_eq!(executor.KVRanges.len(), 1);
    assert_eq!(
        kv::Response::Next(&mut executor, &kv::Context::todo())
            .unwrap()
            .unwrap()
            .GetData(),
        &[1]
    );
    assert_eq!(
        kv::Response::Next(&mut executor, &kv::Context::todo())
            .unwrap()
            .unwrap()
            .GetData(),
        &[2]
    );
    kv::Response::Close(&mut executor).unwrap();
    kv::Response::Close(&mut executor).unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    assert_eq!(registry.registered.lock().unwrap().len(), 1);
    assert_eq!(registry.unregistered.lock().unwrap().len(), 1);
}

/// 可恢复错误：重建 gather、丢弃旧缓冲，Next 得到新脚本数据。
#[test]
fn recoverable_error_rebuilds_with_new_gather_and_discards_held_results() {
    let closes = Arc::new(AtomicUsize::new(0));
    let factory = Arc::new(TestFactory {
        scripts: Mutex::new(
            vec![
                script(vec![Step::Data(vec![9]), Step::Error("retry")]),
                script(vec![Step::Data(vec![3]), Step::End]),
            ]
            .into(),
        ),
        builds: AtomicUsize::new(0),
        closes: closes.clone(),
    });
    let registry = Arc::new(TestRegistry::default());
    let mut parent = memory::tracker::NewTracker(10, 0);
    let mut executor = NewExecutorWithRetry(
        kv::Context::todo(),
        &mut parent,
        kv::MPPQueryID::default(),
        Arc::new(AtomicU64::new(40)),
        factory.clone(),
        registry.clone(),
        MppRecoveryConfig {
            enabled: true,
            holder_capacity: 2,
            ..Default::default()
        },
    )
    .expect("retry executor");
    executor.recovery_handler_mut().handlers = vec![Box::new(AcceptRecovery)];

    let response = kv::Response::Next(&mut executor, &kv::Context::todo())
        .unwrap()
        .unwrap();
    assert_eq!(response.GetData(), &[3]);
    assert_eq!(factory.builds.load(Ordering::SeqCst), 2);
    assert_eq!(executor.gather_id(), 42);
    assert_eq!(registry.registered.lock().unwrap().len(), 2);
    assert_eq!(registry.unregistered.lock().unwrap().len(), 1);
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}
