// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// UniStore MVCC 行为单测与 Go 机械迁移遗留草稿。
//
// 前半大段块注释保留 Go 测试夹具/Must* helper 与场景用例，便于对照；
// 后半可执行用例覆盖乐观预写提交、快照可见性、回滚幂等、一阶段提交与事务状态检查。
// MVCC（多版本并发控制）按时间戳保留多版本，读请求只看见 commit_ts 不大于读时间戳的版本。

// UniStore MVCC 测试如何搭建 badger-backed test store、构造事务请求并断言锁/写入/回滚状态。
//
// pub const maxTs: u64 = u64::MAX;
// pub const lockTTL: u64 = 50;
//
// TestStore 对应 Go 测试夹具：保存 MVCCStore、Server、临时路径和 testing.T。
// Rust 沿用字段形状，用于说明每个 helper 通过同一套 store 上下文发请求。
// pub struct TestStore {
//     pub MvccStore: MVCCStore,
//     pub Svr: Server,
//     pub DBPath: String,
//     pub LogPath: String,
//     pub t: testing::T,
// }
//
// impl TestStore {
// newReqCtx 对应 Go 默认 region key 范围 ['t', 'u')。
//     pub fn newReqCtx(&self) -> requestCtx {
//         self.newReqCtxWithKeys(b("t"), b("u"))
//     }
//
// newReqCtxWithKeys 保留 region epoch、peer、latches 和 rpc context 的构造关系。
//     pub fn newReqCtxWithKeys(&self, rawStartKey: Vec<u8>, rawEndKey: Vec<u8>) -> requestCtx {
//         let epoch = metapb::RegionEpoch { ConfVer: 1, Version: 1 };
//         let peer = metapb::Peer { Id: 1, StoreId: 1, Role: metapb::PeerRole_Voter };
//         requestCtx {
//             regCtx: regionCtx {
//                 meta: metapb::Region {
//                     Id: 1,
//                     RegionEpoch: epoch.clone(),
//                     Peers: vec![peer.clone()],
//                 },
//                 latches: newLatches(),
//                 rawStartKey,
//                 rawEndKey,
//             },
//             rpcCtx: kvrpcpb::Context {
//                 RegionId: 1,
//                 RegionEpoch: epoch,
//                 Peer: peer,
//                 ..Default::default()
//             },
//             svr: self.Svr.clone(),
//         }
//     }
// }
//
// fn b(s: &str) -> Vec<u8> {
//     s.as_bytes().to_vec()
// }
//
// newMutation 对应 Go 的 kvrpcpb.Mutation 构造辅助。
// pub fn newMutation(op: kvrpcpb::Op, key: Vec<u8>, value: Option<Vec<u8>>) -> kvrpcpb::Mutation {
//     kvrpcpb::Mutation { Op: op, Key: key, Value: value.unwrap_or_default(), ..Default::default() }
// }
//
// CreateTestDB 保留 badger managed txn 的测试数据库选项；真实打开动作在这里中仍是占位。
// pub fn CreateTestDB(dbPath: &str, LogPath: &str) -> Result<badger::DB, errors::Error> {
//     let subPath = format!("/{}", 0);
//     let mut opts = badger::DefaultOptions;
//     opts.Dir = format!("{}{}", dbPath, subPath);
//     opts.ValueDir = format!("{}{}", LogPath, subPath);
//     opts.ManagedTxns = true;
//     badger::Open(opts)
// }
//
// NewTestStore 对应 Go 的测试夹具初始化：临时目录、badger DB、lockstore、MockRegionManager、MockPD、MVCCStore 和 Server。
// pub fn NewTestStore(_dbPrefix: &str, _logPrefix: &str, t: &testing::T) -> TestStore {
//     let dbPath = t.TempDir();
//     let LogPath = t.TempDir();
//     let safePoint = SafePoint::default();
//     let db = CreateTestDB(&dbPath, &LogPath).unwrap();
//     let dbBundle = mvcc::DBBundle { DB: db.clone(), LockStore: lockstore::NewMemStore(4096) };
//
// Go 原测试显式创建 kv/raft/snap 目录，覆盖 raft store path 依赖。
//     let kvPath = filepath::Join(&dbPath, "kv");
//     let raftPath = filepath::Join(&dbPath, "raft");
//     let snapPath = filepath::Join(&dbPath, "snap");
//     os::MkdirAll(&kvPath, os::ModePerm).unwrap();
//     os::MkdirAll(&raftPath, os::ModePerm).unwrap();
//     os::Mkdir(&snapPath, os::ModePerm).unwrap();
//     let writer = NewDBWriter(dbBundle.clone());
//
//     let rm = NewMockRegionManager(dbBundle.clone(), 1, RegionOptions {
//         StoreAddr: "127.0.0.1:10086".into(),
//         PDAddr: "127.0.0.1:2379".into(),
//         RegionSize: 96 * 1024 * 1024,
//     }).unwrap();
//     let pdClient = NewMockPD(rm);
//     let store = NewMVCCStore(&config::DefaultConf, dbBundle, &dbPath, &safePoint, writer, pdClient);
//     let svr = NewServer(None, None, store.clone(), None);
//
// Go 的 t.Cleanup 关闭 store 和 db；保留资源收尾语义但不实际接线 Drop。
//     t.Cleanup(|| {
//         store.Close().unwrap();
//         db.Close().unwrap();
//     });
//
//     TestStore { MvccStore: store, Svr: svr, DBPath: dbPath, LogPath, t: t.clone() }
// }
//
// PessimisticLock 对应 Go helper：构造 PessimisticLockRequest 并返回 waiter。
// pub fn PessimisticLock(pk: Vec<u8>, key: Vec<u8>, startTs: u64, lockTTL: u64, forUpdateTs: u64, isFirstLock: bool, forceLock: bool, store: &TestStore) -> Result<lockwaiter::Waiter, errors::Error> {
//     let req = kvrpcpb::PessimisticLockRequest {
//         Mutations: vec![newMutation(kvrpcpb::Op_PessimisticLock, key, None)],
//         PrimaryLock: pk,
//         StartVersion: startTs,
//         LockTtl: lockTTL,
//         ForUpdateTs: forUpdateTs,
//         IsFirstLock: isFirstLock,
//         Force: forceLock,
//         ..Default::default()
//     };
//     store.MvccStore.PessimisticLock(store.newReqCtx(), &req, &kvrpcpb::PessimisticLockResponse::default())
// }
//
// PrewriteOptimistic 保留乐观事务 prewrite 的默认 assertion 配置。
// pub fn PrewriteOptimistic(pk: Vec<u8>, key: Vec<u8>, value: Option<Vec<u8>>, startTs: u64, lockTTL: u64, minCommitTs: u64, useAsyncCommit: bool, secondaries: Vec<Vec<u8>>, store: &TestStore) -> Result<(), errors::Error> {
//     PrewriteOptimisticWithAssertion(pk, key, value, startTs, lockTTL, minCommitTs, useAsyncCommit, secondaries, kvrpcpb::Assertion_None, kvrpcpb::AssertionLevel_Off, store)
// }
//
// PrewriteOptimisticWithAssertion 对应 Go 的 assertion 版本；value 为 nil 时转换为 Op_Del。
// pub fn PrewriteOptimisticWithAssertion(pk: Vec<u8>, key: Vec<u8>, value: Option<Vec<u8>>, startTs: u64, lockTTL: u64, minCommitTs: u64, useAsyncCommit: bool, secondaries: Vec<Vec<u8>>, assertion: kvrpcpb::Assertion, assertionLevel: kvrpcpb::AssertionLevel, store: &TestStore) -> Result<(), errors::Error> {
//     let op = if value.is_none() { kvrpcpb::Op_Del } else { kvrpcpb::Op_Put };
//     let mut mutation = newMutation(op, key, value);
//     mutation.Assertion = assertion;
//     let prewriteReq = kvrpcpb::PrewriteRequest {
//         Mutations: vec![mutation],
//         PrimaryLock: pk,
//         StartVersion: startTs,
//         LockTtl: lockTTL,
//         MinCommitTs: minCommitTs,
//         UseAsyncCommit: useAsyncCommit,
//         Secondaries: secondaries,
//         AssertionLevel: assertionLevel,
//         ..Default::default()
//     };
//     store.MvccStore.prewriteOptimistic(store.newReqCtx(), prewriteReq.Mutations.clone(), &prewriteReq)
// }
//
// pub fn PrewritePessimistic(pk: Vec<u8>, key: Vec<u8>, value: Option<Vec<u8>>, startTs: u64, lockTTL: u64, isPessimisticLock: Vec<bool>, forUpdateTs: u64, store: &TestStore) -> Result<(), errors::Error> {
//     PrewritePessimisticWithAssertion(pk, key, value, startTs, lockTTL, isPessimisticLock, forUpdateTs, kvrpcpb::Assertion_None, kvrpcpb::AssertionLevel_Off, store)
// }
//
// PrewritePessimisticWithAssertion 把 bool slice 映射为 DO/SKIP PESSIMISTIC_CHECK，保留 Go 的逐项转换语义。
// pub fn PrewritePessimisticWithAssertion(pk: Vec<u8>, key: Vec<u8>, value: Option<Vec<u8>>, startTs: u64, lockTTL: u64, isPessimisticLock: Vec<bool>, forUpdateTs: u64, assertion: kvrpcpb::Assertion, assertionLevel: kvrpcpb::AssertionLevel, store: &TestStore) -> Result<(), errors::Error> {
//     let mut mutation = newMutation(kvrpcpb::Op_Put, key, value);
//     mutation.Assertion = assertion;
//     let pessimisticActions = isPessimisticLock.iter().map(|locked| {
//         if *locked { kvrpcpb::PrewriteRequest_DO_PESSIMISTIC_CHECK } else { kvrpcpb::PrewriteRequest_SKIP_PESSIMISTIC_CHECK }
//     }).collect::<Vec<_>>();
//     let prewriteReq = kvrpcpb::PrewriteRequest {
//         Mutations: vec![mutation],
//         PrimaryLock: pk,
//         StartVersion: startTs,
//         LockTtl: lockTTL,
//         PessimisticActions: pessimisticActions,
//         ForUpdateTs: forUpdateTs,
//         AssertionLevel: assertionLevel,
//         ..Default::default()
//     };
//     store.MvccStore.prewritePessimistic(store.newReqCtx(), prewriteReq.Mutations.clone(), &prewriteReq)
// }
//
// pub fn MustCheckTxnStatus(pk: Vec<u8>, lockTs: u64, callerStartTs: u64, currentTs: u64, rollbackIfNotExists: bool, ttl: u64, commitTs: u64, action: kvrpcpb::Action, s: &TestStore) {
//     let (resTTL, resCommitTs, resAction, err) = CheckTxnStatus(pk, lockTs, callerStartTs, currentTs, rollbackIfNotExists, s);
//     assert!(err.is_none());
//     assert_eq!(ttl, resTTL);
//     assert_eq!(commitTs, resCommitTs);
//     assert_eq!(action, resAction);
// }
//
// pub fn CheckTxnStatus(pk: Vec<u8>, lockTs: u64, callerStartTs: u64, currentTs: u64, rollbackIfNotExists: bool, store: &TestStore) -> (u64, u64, kvrpcpb::Action, Option<errors::Error>) {
//     let req = kvrpcpb::CheckTxnStatusRequest {
//         PrimaryKey: pk,
//         LockTs: lockTs,
//         CallerStartTs: callerStartTs,
//         CurrentTs: currentTs,
//         RollbackIfNotExist: rollbackIfNotExists,
//         ..Default::default()
//     };
//     let (txnStatus, err) = store.MvccStore.CheckTxnStatus(store.newReqCtx(), &req);
//     let ttl = txnStatus.lockInfo.as_ref().map(|lock| lock.LockTtl).unwrap_or(0);
//     (ttl, txnStatus.commitTS, txnStatus.action, err)
// }
//
// pub fn CheckSecondaryLocksStatus(keys: Vec<Vec<u8>>, startTS: u64, store: &TestStore) -> (Vec<kvrpcpb::LockInfo>, u64, Option<errors::Error>) {
//     let (status, err) = store.MvccStore.CheckSecondaryLocks(store.newReqCtx(), keys, startTS);
//     (status.locks, status.commitTS, err)
// }
//
// 下列 Must* helper 对应 Go 中 require 断言包装：每个 helper 执行一个 MVCC 动作并立即检查结果。
// pub fn MustLocked(key: Vec<u8>, pessimistic: bool, store: &TestStore) {
//     let lock = store.MvccStore.getLock(store.newReqCtx(), key);
//     assert!(lock.is_some());
//     if pessimistic { assert!(lock.ForUpdateTS > 0); } else { assert_eq!(0_u64, lock.ForUpdateTS); }
// }
//
// pub fn MustPessimisticLocked(key: Vec<u8>, startTs: u64, forUpdateTs: u64, store: &TestStore) {
//     let lock = store.MvccStore.getLock(store.newReqCtx(), key);
//     assert!(lock.is_some());
//     assert_eq!(startTs, lock.StartTS);
//     assert_eq!(forUpdateTs, lock.ForUpdateTS);
// }
//
// pub fn MustUnLocked(key: Vec<u8>, store: &TestStore) {
//     let lock = store.MvccStore.getLock(store.newReqCtx(), key);
//     assert!(lock.is_none());
// }
//
// pub fn MustPrewritePut(pk: Vec<u8>, key: Vec<u8>, val: Vec<u8>, startTs: u64, store: &TestStore) { MustPrewriteOptimistic(pk, key, Some(val), startTs, 50, startTs, store); }
// pub fn MustPrewriteDelete(pk: Vec<u8>, key: Vec<u8>, startTs: u64, store: &TestStore) { MustPrewriteOptimistic(pk, key, None, startTs, 50, startTs, store); }
// pub fn MustAcquirePessimisticLock(pk: Vec<u8>, key: Vec<u8>, startTs: u64, forUpdateTs: u64, store: &TestStore) { assert!(PessimisticLock(pk, key, startTs, lockTTL, forUpdateTs, false, false, store).is_ok()); }
// pub fn MustAcquirePessimisticLockForce(pk: Vec<u8>, key: Vec<u8>, startTs: u64, forUpdateTs: u64, store: &TestStore) { assert!(PessimisticLock(pk, key, startTs, lockTTL, forUpdateTs, false, true, store).is_ok()); }
// pub fn MustAcquirePessimisticLockErr(pk: Vec<u8>, key: Vec<u8>, startTs: u64, forUpdateTs: u64, store: &TestStore) { assert!(PessimisticLock(pk, key, startTs, lockTTL, forUpdateTs, false, false, store).is_err()); }
// pub fn MustPessimisitcPrewriteDelete(pk: Vec<u8>, key: Vec<u8>, startTs: u64, forUpdateTs: u64, store: &TestStore) { MustPrewritePessimistic(pk, key, None, startTs, 5000, vec![true], forUpdateTs, store); }
//
// pub fn MustPessimisticRollback(key: Vec<u8>, startTs: u64, forUpdateTs: u64, store: &TestStore) {
//     let req = kvrpcpb::PessimisticRollbackRequest { StartVersion: startTs, ForUpdateTs: forUpdateTs, Keys: vec![key], ..Default::default() };
//     assert!(store.MvccStore.PessimisticRollback(store.newReqCtx(), &req).is_ok());
// }
//
// pub fn MustPrewriteOptimistic(pk: Vec<u8>, key: Vec<u8>, value: Vec<u8>, startTs: u64, lockTTL: u64, minCommitTs: u64, store: &TestStore) {
//     assert!(PrewriteOptimistic(pk, key.clone(), Some(value.clone()), startTs, lockTTL, minCommitTs, false, vec![], store).is_ok());
//     let lock = store.MvccStore.getLock(store.newReqCtx(), key);
//     assert_eq!(lockTTL, lock.TTL as u64);
//     assert_eq!(value, lock.Value);
// }
//
// pub fn MustPrewriteOptimisticAsyncCommit(pk: Vec<u8>, key: Vec<u8>, value: Vec<u8>, startTs: u64, lockTTL: u64, minCommitTs: u64, secondaries: Vec<Vec<u8>>, store: &TestStore) {
//     assert!(PrewriteOptimistic(pk, key.clone(), Some(value.clone()), startTs, lockTTL, minCommitTs, true, secondaries, store).is_ok());
//     let lock = store.MvccStore.getLock(store.newReqCtx(), key);
//     assert_eq!(lockTTL, lock.TTL as u64);
//     assert_eq!(value, lock.Value);
// }
//
// pub fn MustPrewritePessimisticPut(pk: Vec<u8>, key: Vec<u8>, value: Vec<u8>, startTs: u64, forUpdateTs: u64, store: &TestStore) {
//     MustPrewritePessimistic(pk, key, Some(value), startTs, lockTTL, vec![true], forUpdateTs, store);
// }
//
// pub fn MustPrewritePessimisticDelete(pk: Vec<u8>, key: Vec<u8>, startTs: u64, forUpdateTs: u64, store: &TestStore) {
//     MustPrewritePessimistic(pk, key, None, startTs, lockTTL, vec![true], forUpdateTs, store);
// }
//
// pub fn MustPrewritePessimistic(pk: Vec<u8>, key: Vec<u8>, value: Option<Vec<u8>>, startTs: u64, lockTTL: u64, isPessimisticLock: Vec<bool>, forUpdateTs: u64, store: &TestStore) {
//     assert!(PrewritePessimistic(pk, key.clone(), value.clone(), startTs, lockTTL, isPessimisticLock, forUpdateTs, store).is_ok());
//     let lock = store.MvccStore.getLock(store.newReqCtx(), key);
//     assert_eq!(forUpdateTs, lock.ForUpdateTS);
//     assert_eq!(value.unwrap_or_default(), lock.Value);
// }
//
// pub fn MustPrewritePessimisticPutErr(pk: Vec<u8>, key: Vec<u8>, value: Vec<u8>, startTs: u64, forUpdateTs: u64, store: &TestStore) {
//     assert!(PrewritePessimistic(pk, key, Some(value), startTs, lockTTL, vec![true], forUpdateTs, store).is_err());
// }
//
// pub fn MustPrewritePutLockErr(pk: Vec<u8>, key: Vec<u8>, val: Vec<u8>, startTs: u64, store: &TestStore) { assert!(PrewriteOptimistic(pk, key, Some(val), startTs, lockTTL, startTs, false, vec![], store).is_err()); }
// pub fn MustPrewritePutErr(pk: Vec<u8>, key: Vec<u8>, val: Option<Vec<u8>>, startTs: u64, store: &TestStore) { assert!(PrewriteOptimistic(pk, key, val, startTs, lockTTL, startTs, false, vec![], store).is_err()); }
// pub fn MustPrewriteInsert(pk: Vec<u8>, key: Vec<u8>, val: Vec<u8>, startTs: u64, store: &TestStore) { assert!(PrewriteOptimistic(pk, key, Some(val), startTs, lockTTL, startTs, false, vec![], store).is_ok()); }
// pub fn MustPrewriteInsertAlreadyExists(pk: Vec<u8>, key: Vec<u8>, val: Vec<u8>, startTs: u64, store: &TestStore) { assert!(PrewriteOptimistic(pk, key, Some(val), startTs, lockTTL, startTs, false, vec![], store).is_err()); }
// pub fn MustPrewriteOpCheckExistAlreadyExist(pk: Vec<u8>, key: Vec<u8>, startTs: u64, store: &TestStore) { assert!(store.MvccStore.prewriteOptimistic(store.newReqCtx(), vec![newMutation(kvrpcpb::Op_CheckNotExists, key, None)], &kvrpcpb::PrewriteRequest { PrimaryLock: pk, StartVersion: startTs, LockTtl: lockTTL, MinCommitTs: startTs, ..Default::default() }).is_err()); }
// pub fn MustPrewriteOpCheckExistOk(pk: Vec<u8>, key: Vec<u8>, startTs: u64, store: &TestStore) { assert!(store.MvccStore.prewriteOptimistic(store.newReqCtx(), vec![newMutation(kvrpcpb::Op_CheckNotExists, key.clone(), None)], &kvrpcpb::PrewriteRequest { PrimaryLock: pk, StartVersion: startTs, LockTtl: lockTTL, MinCommitTs: startTs, ..Default::default() }).is_ok()); assert_eq!(0, store.MvccStore.lockStore.Get(key, vec![]).len()); }
//
// pub fn MustCommitKeyPut(key: Vec<u8>, val: Vec<u8>, startTs: u64, commitTs: u64, store: &TestStore) { MustCommit(key.clone(), startTs, commitTs, store); assert_eq!(val, store.newReqCtx().getDBReader().Get(key, commitTs).unwrap().0); }
// pub fn MustCommit(key: Vec<u8>, startTs: u64, commitTs: u64, store: &TestStore) { assert!(store.MvccStore.Commit(store.newReqCtx(), vec![key], startTs, commitTs).is_ok()); }
// pub fn MustCommitErr(key: Vec<u8>, startTs: u64, commitTs: u64, store: &TestStore) { assert!(store.MvccStore.Commit(store.newReqCtx(), vec![key], startTs, commitTs).is_err()); }
// pub fn MustRollbackKey(key: Vec<u8>, startTs: u64, store: &TestStore) { assert!(store.MvccStore.Rollback(store.newReqCtx(), vec![key.clone()], startTs).is_ok()); assert!(store.MvccStore.lockStore.Get(key.clone(), vec![]).is_none()); assert!(store.MvccStore.checkExtraTxnStatus(store.newReqCtx(), key, startTs).isRollback); }
// pub fn MustRollbackErr(key: Vec<u8>, startTs: u64, store: &TestStore) { assert!(store.MvccStore.Rollback(store.newReqCtx(), vec![key], startTs).is_err()); }
// pub fn MustGetNone(key: Vec<u8>, startTs: u64, store: &TestStore) { assert!(MustGet(key, startTs, store).is_empty()); }
// pub fn MustGetVal(key: Vec<u8>, val: Vec<u8>, startTs: u64, store: &TestStore) { assert_eq!(val, MustGet(key, startTs, store)); }
// pub fn MustGetErr(key: Vec<u8>, startTs: u64, store: &TestStore) { assert!(kvGet(key, startTs, vec![], vec![], store).is_err()); }
//
// pub fn kvGet(key: Vec<u8>, readTs: u64, resolved: Vec<u64>, committed: Vec<u64>, store: &TestStore) -> Result<Vec<u8>, errors::Error> {
//     let mut reqCtx = store.newReqCtx();
//     reqCtx.rpcCtx.ResolvedLocks = resolved;
//     reqCtx.rpcCtx.CommittedLocks = committed;
//     store.MvccStore.Get(reqCtx, key, readTs)
// }
//
// pub fn MustGet(key: Vec<u8>, readTs: u64, store: &TestStore) -> Vec<u8> { kvGet(key, readTs, vec![], vec![], store).unwrap() }
// pub fn MustPrewriteLock(pk: Vec<u8>, key: Vec<u8>, startTs: u64, store: &TestStore) { assert!(store.MvccStore.Prewrite(store.newReqCtx(), &kvrpcpb::PrewriteRequest { Mutations: vec![newMutation(kvrpcpb::Op_Lock, key, None)], PrimaryLock: pk, StartVersion: startTs, LockTtl: lockTTL, ..Default::default() }).is_ok()); }
// pub fn MustPrewriteLockErr(pk: Vec<u8>, key: Vec<u8>, startTs: u64, store: &TestStore) { assert!(store.MvccStore.Prewrite(store.newReqCtx(), &kvrpcpb::PrewriteRequest { Mutations: vec![newMutation(kvrpcpb::Op_Lock, key, None)], PrimaryLock: pk, StartVersion: startTs, LockTtl: lockTTL, ..Default::default() }).is_err()); }
// pub fn MustCleanup(key: Vec<u8>, startTs: u64, currentTs: u64, store: &TestStore) { assert!(store.MvccStore.Cleanup(store.newReqCtx(), key, startTs, currentTs).is_ok()); }
// pub fn MustCleanupErr(key: Vec<u8>, startTs: u64, currentTs: u64, store: &TestStore) { assert!(store.MvccStore.Cleanup(store.newReqCtx(), key, startTs, currentTs).is_err()); }
// pub fn MustTxnHeartBeat(pk: Vec<u8>, startTs: u64, adviceTTL: u64, expectedTTL: u64, store: &TestStore) { let ttl = store.MvccStore.TxnHeartBeat(store.newReqCtx(), &kvrpcpb::TxnHeartBeatRequest { PrimaryLock: pk, StartVersion: startTs, AdviseLockTtl: adviceTTL, ..Default::default() }).unwrap(); assert_eq!(expectedTTL, ttl); }
// pub fn MustGetRollback(key: Vec<u8>, ts: u64, store: &TestStore) { assert!(store.MvccStore.checkExtraTxnStatus(store.newReqCtx(), key, ts).isRollback); }
//
// #[test]
// pub fn TestBasicOptimistic() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewriteOptimistic(b("key1"), b("key1"), b("val1"), 1, 200, 0, &store);
//     MustCommitKeyPut(b("key1"), b("val1"), 1, 2, &store);
// 读时间戳小于 commit_ts 时应读不到这条写入。
//     let getVal = store.newReqCtx().getDBReader().Get(b("key1"), 1).unwrap().0;
//     assert!(getVal.is_none());
// }
//
// #[test]
// pub fn TestPessimiticTxnTTL() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     assert!(PessimisticLock(b("key1"), b("key1"), 1, 1000, 1, true, false, &store).is_ok());
//     MustPrewritePessimistic(b("key1"), b("key1"), Some(b("val1")), 1, 500, vec![true], 1, &store);
//     assert_eq!(1000_u64, store.MvccStore.getLock(store.newReqCtx(), b("key1")).TTL as u64);
//     assert!(PessimisticLock(b("key2"), b("key2"), 3, 300, 3, true, false, &store).is_ok());
//     MustPrewritePessimistic(b("key2"), b("key2"), Some(b("val2")), 3, 2000, vec![true], 3, &store);
//     assert_eq!(2000_u64, store.MvccStore.getLock(store.newReqCtx(), b("key2")).TTL as u64);
// }
//
// #[test]
// pub fn TestRollback() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewriteOptimistic(b("tkey"), b("tkey"), b("value"), 1, 100, 0, &store);
//     MustRollbackKey(b("tkey"), 1, &store);
//     MustPrewriteOptimistic(b("tkey"), b("tkey"), b("value"), 2, 100, 0, &store);
//     MustRollbackKey(b("tkey"), 2, &store);
//     assert!(store.MvccStore.checkExtraTxnStatus(store.newReqCtx(), b("tkey"), 1).isRollback);
// Go 明确说明 TiKV 会 collapse rollback，但 unistore 保留两个 rollback 记录。
//     MustPrewritePut(b("tk"), b("tk"), b("v"), 1, &store);
//     MustRollbackKey(b("tk"), 1, &store);
//     MustGetRollback(b("tk"), 1, &store);
//     MustPrewritePut(b("tk"), b("tk"), b("v"), 2, &store);
//     MustRollbackKey(b("tk"), 2, &store);
//     MustGetNone(b("tk"), 2, &store);
//     MustGetRollback(b("tk"), 2, &store);
//     MustGetRollback(b("tk"), 1, &store);
// }
//
// #[test]
// pub fn TestOverwritePessimisitcLock() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     assert!(PessimisticLock(b("key"), b("key"), 1, 100, 100, true, false, &store).is_ok());
//     assert_eq!(100_u64, store.MvccStore.getLock(store.newReqCtx(), b("key")).ForUpdateTS);
//     assert!(PessimisticLock(b("key"), b("key"), 1, 100, 107, true, false, &store).is_ok());
//     assert_eq!(107_u64, store.MvccStore.getLock(store.newReqCtx(), b("key")).ForUpdateTS);
//     assert!(PessimisticLock(b("key"), b("key"), 1, 100, 93, true, false, &store).is_ok());
//     assert_eq!(107_u64, store.MvccStore.getLock(store.newReqCtx(), b("key")).ForUpdateTS);
// }
//
// #[test]
// pub fn TestCheckTxnStatus() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     let (ttl, commit_ts, action, err) = CheckTxnStatus(b("tpk"), 1, 3, 5, true, &store);
//     assert_eq!(0, ttl);
//     assert_eq!(0, commit_ts);
//     assert_eq!(kvrpcpb::Action_LockNotExistRollback, action);
//     assert!(err.is_none());
//
// checkTxnStatus 先写 rollback，随后同 start_ts 的 prewrite 必须 AlreadyRollback。
//     assert!(PrewriteOptimistic(b("tpk"), b("tpk"), Some(b("val")), 1, 100, 20, false, vec![], &store).is_err());
//     MustPrewriteOptimistic(b("tpk"), b("tpk"), b("val"), 2, 100, 20, &store);
//     MustCheckTxnStatus(b("tpk"), 2, 3, 5, true, 100, 0, kvrpcpb::Action_MinCommitTSPushed, &store);
//     MustCheckTxnStatus(b("tpk"), 2, 25, 25, true, 100, 0, kvrpcpb::Action_MinCommitTSPushed, &store);
//     assert_eq!(26_u64, store.MvccStore.getLock(store.newReqCtx(), b("tpk")).MinCommitTS);
//     MustCommitErr(b("tpk"), 2, 35, &store);
//     MustCommitKeyPut(b("tpk"), b("val"), 2, 41, &store);
//     MustCheckTxnStatus(b("tpk"), 2, 42, 42, true, 0, 41, kvrpcpb::Action_NoAction, &store);
//
// primary mismatch 路径通过另一个 primary 加悲观锁后检查原 key。
//     MustAcquirePessimisticLock(b("another_key"), b("tpk"), 43, 43, &store);
//     let (_, _, _, err) = CheckTxnStatus(b("tpk"), 43, 44, 44, true, &store);
//     assert!(errors::Cause(err).is::<kverrors::ErrPrimaryMismatch>());
//     MustPessimisticRollback(b("tpk"), 43, 43, &store);
// }
//
// #[test]
// pub fn TestCheckSecondaryLocksStatus() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewritePut(b("pk"), b("secondary"), b("val"), 1, &store);
//     MustCommit(b("secondary"), 1, 3, &store);
//     MustRollbackKey(b("secondary"), 5, &store);
//     MustPrewritePut(b("pk"), b("secondary"), b("val"), 7, &store);
//     MustCommit(b("secondary"), 7, 9, &store);
//
// 已提交、已 rollback、无提交信息、悲观锁和乐观锁五种 secondary lock 状态都按 Go 顺序保留。
//     assert_eq!(3_u64, CheckSecondaryLocksStatus(vec![b("secondary")], 1, &store).1);
//     assert_eq!(9_u64, CheckSecondaryLocksStatus(vec![b("secondary")], 7, &store).1);
//     MustGetRollback(b("secondary"), 5, &store);
//     MustGetRollback(b("secondary"), 6, &store);
//     MustAcquirePessimisticLock(b("pk"), b("secondary"), 11, 11, &store);
//     assert!(CheckSecondaryLocksStatus(vec![b("secondary")], 11, &store).0.is_empty());
//     MustGetRollback(b("secondary"), 11, &store);
//     MustPrewritePut(b("pk"), b("secondary"), b("val"), 13, &store);
//     assert_eq!(1, CheckSecondaryLocksStatus(vec![b("secondary")], 13, &store).0.len());
//     MustLocked(b("secondary"), false, &store);
// }
//
// #[test]
// pub fn TestMvccGet() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewriteOptimistic(b("t1_r1"), b("t1_r1"), b("pkVal"), 1, 100, 0, &store);
//     MustCommitKeyPut(b("t1_r1"), b("pkVal"), 1, 2, &store);
//     MustPrewriteOptimistic(b("t1_r1"), b("t1_r1"), b("aba"), 3, 100, 0, &store);
//     MustCommitKeyPut(b("t1_r1"), b("aba"), 3, 4, &store);
//     assert_eq!(2, store.MvccStore.MvccGetByKey(store.newReqCtx(), b("t1_r1")).unwrap().Writes.len());
//     MustPrewriteOptimistic(b("t1_r1"), b("t1_r1"), b("rollbackVal"), 5, 100, 0, &store);
//     MustRollbackKey(b("t1_r1"), 5, &store);
//     MustPrewriteOptimistic(b("t1_r1"), b("t1_r1"), b(""), 7, 100, 0, &store);
//     MustCommitKeyPut(b("t1_r1"), b(""), 7, 8, &store);
//     let res = store.MvccStore.MvccGetByKey(store.newReqCtx(), b("t1_r1")).unwrap();
//     assert_eq!(4, res.Writes.len());
// MvccGetByStartTs 需要能按 start_ts 找回 key，并受 region key 范围过滤。
//     assert!(store.MvccStore.MvccGetByStartTs(store.newReqCtx(), 7).unwrap().0.is_some());
//     assert!(store.MvccStore.MvccGetByStartTs(store.newReqCtx(), 1000).unwrap().0.is_none());
//     assert!(store.MvccStore.MvccGetByStartTs(store.newReqCtxWithKeys(b("t1_r1"), b("t1_r2")), 3).unwrap().0.is_some());
// }
//
// #[test]
// pub fn TestPrimaryKeyOpLock() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewriteLock(b("tpk"), b("tpk"), 100, &store);
//     MustCommit(b("tpk"), 100, 101, &store);
//     assert_eq!(101_u64, CheckTxnStatus(b("tpk"), 100, 110, 110, false, &store).1);
//     MustPrewriteOptimistic(b("tpk"), b("tpk"), b("val2"), 110, 100, 0, &store);
//     MustCommit(b("tpk"), 110, 111, &store);
//     MustPrewriteLock(b("tpk"), b("tpk"), 120, &store);
//     MustCommit(b("tpk"), 120, 121, &store);
//     assert_eq!(121_u64, CheckTxnStatus(b("tpk"), 120, 130, 130, false, &store).1);
//     assert_eq!(111_u64, CheckTxnStatus(b("tpk"), 110, 130, 130, false, &store).1);
//     assert_eq!(101_u64, CheckTxnStatus(b("tpk"), 100, 130, 130, false, &store).1);
//     MustGetVal(b("tpk"), b("val2"), 111, &store);
//     MustGetVal(b("tpk"), b("val2"), 130, &store);
// }
//
// #[test]
// pub fn TestMvccTxnRead() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustGetNone(b("tk1"), 1, &store);
//     MustPrewriteOptimistic(b("tk1"), b("tk1"), b("v1"), 2, 10, 2, &store);
//     MustRollbackKey(b("tk1"), 2, &store);
//     MustGetNone(b("tk1"), 1, &store);
//     MustPrewriteLock(b("tk1"), b("tk1"), 3, &store);
//     MustCommit(b("tk1"), 3, 4, &store);
//     MustGetNone(b("tk1"), 5, &store);
//     MustPrewriteOptimistic(b("tk1"), b("tk1"), b("v"), 5, 10, 5, &store);
//     MustPrewriteOptimistic(b("tk1"), b("tk2"), b("v2"), 5, 10, 5, &store);
//     MustGetNone(b("tk1"), 4, &store);
//     MustGetErr(b("tk1"), 7, &store);
//     MustGetNone(b("tk1"), maxTs, &store);
//     MustGetErr(b("tk2"), maxTs, &store);
//     MustCommit(b("tk1"), 5, 10, &store);
//     MustCommit(b("tk2"), 5, 10, &store);
//     MustGetVal(b("tk1"), b("v"), 13, &store);
//     MustGetVal(b("tk2"), b("v2"), maxTs, &store);
//     MustPrewriteDelete(b("tk1"), b("tk1"), 15, &store);
//     MustGetVal(b("tk1"), b("v"), maxTs, &store);
//     MustCommit(b("tk1"), 15, 20, &store);
//     MustGetNone(b("tk1"), 23, &store);
// 交错时间戳悲观事务：读 30 可见旧值，悲观 delete prewrite 后同时间读到锁冲突。
//     MustPrewritePut(b("tk1"), b("tk1"), b("v"), 25, &store);
//     MustCommit(b("tk1"), 25, 27, &store);
//     MustAcquirePessimisticLock(b("tk1"), b("tk1"), 23, 29, &store);
//     MustGetVal(b("tk1"), b("v"), 30, &store);
//     MustPessimisitcPrewriteDelete(b("tk1"), b("tk1"), 23, 29, &store);
//     MustGetErr(b("tk1"), 30, &store);
//     MustGetVal(b("tk1"), b("v"), maxTs, &store);
//     MustCommit(b("tk1"), 23, 31, &store);
//     MustGetNone(b("tk1"), 32, &store);
// }
//
// #[test]
// pub fn TestTxnPrewrite() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewritePut(b("tk"), b("tk"), b("v"), 5, &store);
//     MustLocked(b("tk"), false, &store);
//     MustPrewritePut(b("tk"), b("tk"), b("v"), 5, &store);
//     MustPrewritePutLockErr(b("tk"), b("tk"), b("v"), 6, &store);
//     MustCommit(b("tk"), 5, 10, &store);
//     MustGetVal(b("tk"), b("v"), 10, &store);
//     MustPrewritePutErr(b("tk"), b("tk"), Some(b("v")), 5, &store);
//     MustUnLocked(b("tk"), &store);
//     MustPrewritePutErr(b("tk"), b("tk"), Some(b("v")), 6, &store);
//     MustPrewriteLock(b("tk"), b("tk"), 12, &store);
//     MustRollbackKey(b("tk"), 12, &store);
//     MustPrewritePutErr(b("tk"), b("tk"), None, 12, &store);
//     MustPrewriteDelete(b("tk"), b("tk"), 13, &store);
//     MustRollbackKey(b("tk"), 13, &store);
// }
//
// #[test]
// pub fn TestPrewriteInsert() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewritePut(b("tk1"), b("tk1"), b("v1"), 1, &store);
//     MustCommit(b("tk1"), 1, 2, &store);
//     MustPrewriteInsertAlreadyExists(b("tk1"), b("tk1"), b("v2"), 3, &store);
//     MustPrewriteDelete(b("tk1"), b("tk1"), 4, &store);
//     MustCommit(b("tk1"), 4, 5, &store);
//     MustPrewriteInsert(b("tk1"), b("tk1"), b("v2"), 6, &store);
//     MustCommit(b("tk1"), 6, 7, &store);
//     MustPrewritePut(b("tk1"), b("tk1"), b("v3"), 8, &store);
//     MustRollbackKey(b("tk1"), 8, &store);
//     MustPrewriteInsertAlreadyExists(b("tk1"), b("tk1"), b("v2"), 9, &store);
//     MustPrewriteDelete(b("tk1"), b("tk1"), 10, &store);
//     MustCommit(b("tk1"), 10, 11, &store);
//     MustPrewritePut(b("tk1"), b("tk1"), b("v3"), 12, &store);
//     MustRollbackKey(b("tk1"), 12, &store);
//     MustPrewriteInsert(b("tk1"), b("tk1"), b("v2"), 13, &store);
//     MustCommit(b("tk1"), 13, 14, &store);
//     MustGetVal(b("tk1"), b("v2"), 15, &store);
// }
//
// #[test]
// pub fn TestRollbackKey() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewritePut(b("tk"), b("tk"), b("v"), 5, &store);
//     MustCommit(b("tk"), 5, 10, &store);
//     MustPrewriteLock(b("tk"), b("tk"), 15, &store);
//     MustLocked(b("tk"), false, &store);
//     MustRollbackKey(b("tk"), 15, &store);
//     MustGetVal(b("tk"), b("v"), 16, &store);
//     MustPrewriteDelete(b("tk"), b("tk"), 17, &store);
//     MustRollbackKey(b("tk"), 17, &store);
//     MustGetVal(b("tk"), b("v"), 18, &store);
// }
//
// #[test]
// pub fn TestCleanup() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewritePut(b("tk"), b("tk"), b("v"), 10, &store);
//     MustTxnHeartBeat(b("tk"), 10, 100, 100, &store);
//     MustTxnHeartBeat(b("tk"), 10, 90, 100, &store);
//     MustCleanupErr(b("tk"), 10, 20, &store);
//     MustLocked(b("tk"), false, &store);
//     MustCleanup(b("tk"), 11, 20, &store);
//     MustCleanup(b("tk"), 10, 120 << 18, &store);
//     MustUnLocked(b("tk"), &store);
// }
//
// #[test]
// pub fn TestCommit() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustCommitErr(b("tk"), 1, 2, &store);
//     MustPrewritePut(b("tk"), b("tk"), b("v"), 5, &store);
//     MustCommitErr(b("tk"), 4, 5, &store);
//     MustRollbackKey(b("tk"), 5, &store);
//     MustCommitErr(b("tk"), 5, 6, &store);
//     MustPrewritePut(b("tk1"), b("tk1"), b("v"), 10, &store);
//     MustPrewriteLock(b("tk1"), b("tk2"), 10, &store);
//     MustPrewriteDelete(b("tk1"), b("tk3"), 10, &store);
//     MustCommit(b("tk1"), 10, 15, &store);
//     MustCommit(b("tk2"), 10, 15, &store);
//     MustCommit(b("tk3"), 10, 15, &store);
//     MustGetVal(b("tk1"), b("v"), 16, &store);
//     MustGetNone(b("tk2"), 16, &store);
//     MustGetNone(b("tk3"), 16, &store);
//     MustCommit(b("tk1"), 10, 15, &store);
//     MustCommit(b("tk3"), 10, 15, &store);
//     MustRollbackErr(b("tk1"), 10, &store);
//     MustRollbackKey(b("tkr"), 5, &store);
//     MustPrewriteLockErr(b("tkr"), b("tkr"), 5, &store);
// }
//
// #[test]
// pub fn TestMinCommitTs() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewriteOptimistic(b("tk"), b("tk"), b("v"), 10, 100, 11, &store);
//     MustCheckTxnStatus(b("tk"), 10, 20, 20, false, 100, 0, kvrpcpb::Action_MinCommitTSPushed, &store);
//     MustCommitErr(b("tk"), 10, 15, &store);
//     MustCommitErr(b("tk"), 10, 20, &store);
//     MustCommit(b("tk"), 10, 21, &store);
//     MustPrewriteOptimistic(b("tk"), b("tk"), b("v"), 30, 100, 30, &store);
//     MustCheckTxnStatus(b("tk"), 30, 40, 40, false, 100, 0, kvrpcpb::Action_MinCommitTSPushed, &store);
//     MustCommit(b("tk"), 30, 50, &store);
// }
//
// #[test]
// pub fn TestPessimisticLock() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 1, 1, &store);
//     MustPrewritePessimistic(b("tk"), b("tk"), Some(b("v")), 1, 100, vec![true], 1, &store);
//     MustCommit(b("tk"), 1, 2, &store);
//     MustPrewritePut(b("tk"), b("tk"), b("v"), 3, &store);
//     MustAcquirePessimisticLockErr(b("tk"), b("tk"), 4, 4, &store);
//     MustCleanup(b("tk"), 3, 0, &store);
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 5, 5, &store);
//     MustPrewriteLockErr(b("tk"), b("tk"), 6, &store);
//     MustCleanup(b("tk"), 5, 0, &store);
//     MustPrewritePut(b("tk"), b("tk"), b("v"), 7, &store);
//     MustCommit(b("tk"), 7, 9, &store);
//     MustPrewriteLockErr(b("tk"), b("tk"), 8, &store);
//     MustAcquirePessimisticLockErr(b("tk"), b("tk"), 8, 8, &store);
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 8, 9, &store);
//     MustPrewritePessimisticPut(b("tk"), b("tk"), b("v"), 8, 8, &store);
//     MustCommit(b("tk"), 8, 10, &store);
// 后半段覆盖 rollback、重复加锁、读不被悲观锁阻塞、不同 for_update_ts 和 LockTypeNotMatch。
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 11, 11, &store);
//     MustCleanup(b("tk"), 11, 0, &store);
//     MustAcquirePessimisticLockErr(b("tk"), b("tk"), 11, 11, &store);
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 13, 13, &store);
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 13, 13, &store);
//     MustPrewritePessimisticPut(b("tk"), b("tk"), b("v3"), 13, 13, &store);
//     MustCommit(b("tk"), 13, 14, &store);
//     MustGetVal(b("tk"), b("v3"), 15, &store);
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 15, 15, &store);
//     MustGetVal(b("tk"), b("v3"), 16, &store);
//     MustPrewritePessimisticDelete(b("tk"), b("tk"), 15, 15, &store);
//     MustGetErr(b("tk"), 16, &store);
//     MustCommit(b("tk"), 15, 17, &store);
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 35, 36, &store);
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 35, 35, &store);
//     MustPessimisticLocked(b("tk"), 35, 36, &store);
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 35, 37, &store);
//     MustPrewritePessimisticPutErr(b("tk"), b("tk"), b("vvv"), 36, 36, &store);
//     MustPrewritePessimisticPut(b("tk"), b("tk"), b("vvv"), 35, 37, &store);
//     MustCommit(b("tk"), 35, 36, &store);
// Go 注释指出这里未检查 commit_ts < for_update_ts，当前仍会提交成功。
//     MustAcquirePessimisticLock(b("tk"), b("tk"), 40, 40, &store);
//     assert!(PrewriteOptimistic(b("tk"), b("tk"), Some(b("vvv")), 40, lockTTL, 40, false, vec![], &store).is_ok());
//     MustCommit(b("tk"), 40, 41, &store);
// }
//
// #[test]
// pub fn TestResolveCommit() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustAcquirePessimisticLock(b("tpk"), b("tpk"), 1, 1, &store);
//     MustAcquirePessimisticLock(b("tpk"), b("tsk"), 1, 1, &store);
//     MustPrewritePessimistic(b("tpk"), b("tpk"), Some(b("v")), 1, 100, vec![true], 1, &store);
//     MustPrewritePessimistic(b("tpk"), b("tsk"), Some(b("v")), 1, 100, vec![true], 1, &store);
//     MustCommit(b("tpk"), 1, 2, &store);
//     assert!(store.MvccStore.ResolveLock(store.newReqCtx(), vec![b("tsk")], 2, 3).is_ok());
//     assert!(store.MvccStore.getLock(store.newReqCtx(), b("tsk")).is_some());
//     assert!(store.MvccStore.ResolveLock(store.newReqCtx(), vec![b("tsk")], 1, 2).is_ok());
//     MustCommit(b("tsk"), 1, 2, &store);
//     MustAcquirePessimisticLock(b("tk2"), b("tk2"), 3, 3, &store);
//     MustCommit(b("tsk"), 1, 2, &store);
//     MustPrewritePessimistic(b("tk2"), b("tk2"), Some(b("v2")), 3, 100, vec![true], 3, &store);
//     MustCommit(b("tk2"), 3, 4, &store);
// error path: Go 手动写入 badger delete entry 后验证 commit 错误分支。
//     let kvTxn = store.MvccStore.db.NewTransaction(true);
//     let mut e = badger::Entry { Key: y::KeyWithTs(b("tsk"), 3), ..Default::default() };
//     e.SetDelete();
//     kvTxn.SetEntry(e).unwrap();
//     kvTxn.Commit().unwrap();
//     MustCommitErr(b("tsk"), 1, 3, &store);
//     MustAcquirePessimisticLock(b("tsk"), b("tsk"), 5, 5, &store);
//     MustCommitErr(b("tsk"), 1, 3, &store);
// }
//
// pub fn MustLoad(startTS: u64, commitTS: u64, store: &TestStore, pairs: &[&str]) {
//     let mut keys = vec![];
//     let mut vals = vec![];
//     for pair in pairs {
//         let parts = pair.split(':').collect::<Vec<_>>();
//         keys.push(b(parts[0]));
//         vals.push(b(parts[1]));
//     }
//     for i in 0..keys.len() { MustPrewritePut(keys[0].clone(), keys[i].clone(), vals[i].clone(), startTS, store); }
//     for key in keys { MustCommit(key, startTS, commitTS, store); }
// }
//
// #[test]
// pub fn TestBatchGet() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustLoad(100, 101, &store, &["ta:1", "tb:2", "tc:3"]);
//     MustPrewritePut(b("ta"), b("ta"), b("0"), 103, &store);
//     let pairs = store.MvccStore.BatchGet(store.newReqCtx(), vec![b("ta"), b("tb"), b("tc")], 104);
//     assert_eq!(3, pairs.len());
//     assert!(pairs[0].Error.is_some());
//     assert_eq!(b("2"), pairs[1].Value);
//     assert_eq!(b("3"), pairs[2].Value);
// }
//
// #[test]
// pub fn TestCommitPessimisticLock() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustAcquirePessimisticLock(b("ta"), b("ta"), 10, 10, &store);
//     MustCommitErr(b("ta"), 20, 30, &store);
//     MustCommit(b("ta"), 10, 20, &store);
//     MustGet(b("ta"), 30, &store);
// }
//
// #[test]
// pub fn TestOpCheckNotExist() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewritePut(b("ta"), b("ta"), b("v"), 1, &store);
//     MustCommit(b("ta"), 1, 2, &store);
//     MustPrewriteOpCheckExistAlreadyExist(b("ta"), b("ta"), 3, &store);
//     MustPrewriteDelete(b("ta"), b("ta"), 4, &store);
//     MustCommit(b("ta"), 4, 5, &store);
//     MustPrewriteOpCheckExistOk(b("ta"), b("ta"), 6, &store);
//     MustPrewritePut(b("ta"), b("ta"), b("v"), 7, &store);
//     MustRollbackKey(b("ta"), 7, &store);
//     MustPrewriteOpCheckExistOk(b("ta"), b("ta"), 8, &store);
// }
//
// #[test]
// pub fn TestPessimisticLockForce() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewritePut(b("ta"), b("ta"), b("v"), 5, &store);
//     MustCommit(b("ta"), 5, 10, &store);
//     MustAcquirePessimisticLockForce(b("ta"), b("ta"), 1, 1, &store);
//     MustPrewritePessimisticPut(b("ta"), b("ta"), b("v2"), 1, 10, &store);
//     MustCommit(b("ta"), 1, 11, &store);
//     MustGetVal(b("ta"), b("v2"), 13, &store);
// }
//
// #[test]
// pub fn TestScanSampleStep() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     for i in 0..1000 {
//         let k = genScanSampleStepKey(i);
//         MustPrewritePut(k.clone(), k.clone(), k.clone(), 1, &store);
//         MustCommit(k, 1, 2, &store);
//     }
//     let mut scanReq = kvrpcpb::ScanRequest { StartKey: genScanSampleStepKey(100), EndKey: genScanSampleStepKey(900), Limit: 100, Version: 2, SampleStep: 10, ..Default::default() };
//     let mut pairs = store.MvccStore.Scan(store.newReqCtx(), &scanReq);
//     assert_eq!(80, pairs.len());
//     for (i, pair) in pairs.iter().enumerate() { assert_eq!(genScanSampleStepKey(100 + i * 10), pair.Key); }
//     scanReq.Limit = 20;
//     pairs = store.MvccStore.Scan(store.newReqCtx(), &scanReq);
//     assert_eq!(20, pairs.len());
// }
//
// pub fn genScanSampleStepKey(i: usize) -> Vec<u8> {
//     format!("t{:0>4}", i).into_bytes()
// }
//
// #[test]
// pub fn TestAsyncCommitPrewrite() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustPrewriteOptimisticAsyncCommit(b("tpk"), b("tpk"), b("tpkVal"), 1, 100, 0, vec![b("tSecKey1"), b("tSecKey2")], &store);
//     MustPrewriteOptimisticAsyncCommit(b("tpk"), b("tSecKey1"), b("secVal1"), 1, 100, 0, vec![], &store);
//     MustPrewriteOptimisticAsyncCommit(b("tpk"), b("tSecKey2"), b("secVal2"), 1, 100, 0, vec![], &store);
//     let pkLock = store.MvccStore.getLock(store.newReqCtx(), b("tpk"));
//     assert_eq!(2_u32, pkLock.LockHdr.SecondaryNum);
//     assert_eq!(b("tSecKey1"), pkLock.Secondaries[0]);
//     assert_eq!(b("tSecKey2"), pkLock.Secondaries[1]);
//     assert!(pkLock.UseAsyncCommit);
//     let secLock = store.MvccStore.getLock(store.newReqCtx(), b("tSecKey2"));
//     assert_eq!(0_u32, secLock.LockHdr.SecondaryNum);
//     assert!(secLock.Secondaries.is_empty());
//     assert!(secLock.UseAsyncCommit);
//     assert_eq!(b("secVal2"), secLock.Value);
// }
//
// #[test]
// pub fn TestAccessCommittedLocks() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustLoad(10, 20, &store, &["t0:v0"]);
//     MustPrewriteDelete(b("t0"), b("t0"), 30, &store);
//     MustGetErr(b("t0"), 40, &store);
//     assert!(kvGet(b("t0"), 40, vec![20], vec![], &store).is_err());
//     assert!(kvGet(b("t0"), 40, vec![20], vec![20], &store).is_err());
//     assert_eq!(b("v0"), kvGet(b("t0"), 40, vec![30], vec![], &store).unwrap());
//     assert!(kvGet(b("t0"), 40, vec![], vec![30], &store).unwrap().is_empty());
//     MustPrewritePut(b("t1"), b("t1"), b("v1"), 50, &store);
//     assert!(kvGet(b("t1"), 60, vec![50], vec![], &store).unwrap().is_empty());
//     assert_eq!(b("v1"), kvGet(b("t1"), 60, vec![], vec![50], &store).unwrap());
//     MustPrewritePut(b("t2"), b("t2"), b("v2"), 70, &store);
//     MustPrewritePut(b("t3"), b("t3"), b("v3"), 80, &store);
//     MustLoad(80, 90, &store, &["t4:v4"]);
// BatchGet 与 Scan 共用 reqCtx 的 ResolvedLocks/CommittedLocks，验证锁被忽略或访问的路径一致。
//     let mut reqCtx = store.newReqCtx();
//     reqCtx.rpcCtx.ResolvedLocks = vec![80];
//     reqCtx.rpcCtx.CommittedLocks = vec![30, 50];
//     let pairs = store.MvccStore.BatchGet(reqCtx.clone(), vec![b("t0"), b("t1"), b("t2"), b("t3"), b("t4")], 100);
//     assert_eq!(3, pairs.len());
//     let scanReq = kvrpcpb::ScanRequest { StartKey: b("t0"), EndKey: b("t5"), Limit: 100, Version: 100, ..Default::default() };
//     assert_eq!(pairs.len(), store.MvccStore.Scan(reqCtx, &scanReq).len());
// }
//
// #[test]
// pub fn TestTiKVRCRead() {
//     let store = NewTestStore("basic_optimistic_db", "basic_optimistic_log", &testing::T::default());
//     MustLoad(10, 20, &store, &["t1:v1", "t2:v2", "t3:v3"]);
//     MustPrewritePut(b("t1"), b("t1"), b("v11"), 30, &store);
//     MustCommit(b("t1"), 30, 40, &store);
//     MustPrewritePut(b("t2"), b("t2"), b("v2"), 50, &store);
//     MustPrewriteDelete(b("t3"), b("t3"), 60, &store);
//     MustPrewritePut(b("t4"), b("t4"), b("v4"), 70, &store);
//     let mut reqCtx = store.newReqCtx();
//     reqCtx.rpcCtx.IsolationLevel = kvrpcpb::IsolationLevel_RC;
//     assert_eq!(b("v11"), store.MvccStore.Get(reqCtx.clone(), b("t1"), 80).unwrap());
//     assert_eq!(b("v2"), store.MvccStore.Get(reqCtx.clone(), b("t2"), 80).unwrap());
//     assert_eq!(b("v3"), store.MvccStore.Get(reqCtx.clone(), b("t3"), 80).unwrap());
//     assert!(store.MvccStore.Get(reqCtx.clone(), b("t4"), 80).unwrap().is_empty());
//     assert_eq!(3, store.MvccStore.BatchGet(reqCtx.clone(), vec![b("t1"), b("t2"), b("t3"), b("t4")], 80).len());
//     assert_eq!(3, store.MvccStore.Scan(reqCtx, &kvrpcpb::ScanRequest { StartKey: b("t1"), EndKey: b("t4"), Limit: 100, Version: 80, ..Default::default() }).len());
// }
//
// #[test]
// pub fn TestAssertion() {
//     let store = NewTestStore("TestAssertion", "TestAssertion", &testing::T::default());
//     MustPrewriteOptimistic(b("k1"), b("k1"), b("v1"), 1, 100, 0, &store);
//     MustPrewriteOptimistic(b("k1"), b("k2"), b("v2"), 1, 100, 0, &store);
//     MustPrewriteOptimistic(b("k1"), b("k3"), b("v3"), 1, 100, 0, &store);
//     MustCommit(b("k1"), 1, 2, &store);
//     MustCommit(b("k2"), 1, 2, &store);
//     MustCommit(b("k3"), 1, 2, &store);
//
// Go 的闭包会在 assertion 未关闭时检查 ErrAssertionFailed 中的 start/key/assertion/existing ts。
//     for disable in [false, true] {
//         let level = if disable { kvrpcpb::AssertionLevel_Off } else { kvrpcpb::AssertionLevel_Strict };
//         let err = PrewriteOptimisticWithAssertion(b("k1"), b("k1"), Some(b("v1")), 10, 100, 0, false, vec![], kvrpcpb::Assertion_NotExist, level, &store);
//         checkAssertionFailedError(err, disable, 10, b("k1"), kvrpcpb::Assertion_NotExist, 1, 2);
//         let err = PrewriteOptimisticWithAssertion(b("k11"), b("k11"), Some(b("v11")), 10, 100, 0, false, vec![], kvrpcpb::Assertion_Exist, level, &store);
//         checkAssertionFailedError(err, disable, 10, b("k11"), kvrpcpb::Assertion_Exist, 0, 0);
//         MustAcquirePessimisticLock(b("k2"), b("k2"), 10, 10, &store);
//         let err = PrewritePessimisticWithAssertion(b("k2"), b("k2"), Some(b("v2")), 10, 100, vec![true], 10, kvrpcpb::Assertion_NotExist, level, &store);
//         checkAssertionFailedError(err, disable, 10, b("k2"), kvrpcpb::Assertion_NotExist, 1, 2);
//         let err = PrewritePessimisticWithAssertion(b("pk"), b("k3"), Some(b("v3")), 10, 100, vec![false], 10, kvrpcpb::Assertion_NotExist, level, &store);
//         checkAssertionFailedError(err, disable, 10, b("k3"), kvrpcpb::Assertion_NotExist, 1, 2);
//     }
//
//     for k in [b("k1"), b("k11"), b("k2"), b("k22"), b("k3"), b("k33")] {
//         MustRollbackKey(k, 10, &store);
//     }
//
// 通过路径：exist/not-exist assertion 在乐观、悲观和非悲观锁路径都应成功。
//     assert!(PrewriteOptimisticWithAssertion(b("k1"), b("k1"), Some(b("v1")), 20, 100, 0, false, vec![], kvrpcpb::Assertion_Exist, kvrpcpb::AssertionLevel_Strict, &store).is_ok());
//     assert!(PrewriteOptimisticWithAssertion(b("k11"), b("k11"), Some(b("v11")), 20, 100, 0, false, vec![], kvrpcpb::Assertion_NotExist, kvrpcpb::AssertionLevel_Strict, &store).is_ok());
//     MustAcquirePessimisticLock(b("k2"), b("k2"), 20, 10, &store);
//     assert!(PrewritePessimisticWithAssertion(b("k2"), b("k2"), Some(b("v2")), 20, 100, vec![true], 10, kvrpcpb::Assertion_Exist, kvrpcpb::AssertionLevel_Strict, &store).is_ok());
//     assert!(PrewritePessimisticWithAssertion(b("pk"), b("k33"), Some(b("v33")), 20, 100, vec![false], 10, kvrpcpb::Assertion_NotExist, kvrpcpb::AssertionLevel_Strict, &store).is_ok());
// }
//
// pub fn checkAssertionFailedError(err: Result<(), errors::Error>, disable: bool, startTs: u64, key: Vec<u8>, assertion: kvrpcpb::Assertion, existingStartTs: u64, existingCommitTs: u64) {
//     if disable {
//         assert!(err.is_ok());
//         return;
//     }
//     let e = errors::Cause(err.unwrap_err()).downcast::<kverrors::ErrAssertionFailed>().unwrap();
//     assert_eq!(startTs, e.StartTS);
//     assert_eq!(key, e.Key);
//     assert_eq!(assertion, e.Assertion);
//     assert_eq!(existingStartTs, e.ExistingStartTS);
//     assert_eq!(existingCommitTs, e.ExistingCommitTS);
// }
//
// pub fn getConflictErr(res: Vec<kvrpcpb::KvPair>) -> Option<kvrpcpb::WriteConflict> {
//     for pair in res {
//         if pair.Error.is_some() && pair.Error.Conflict.is_some() {
//             return pair.Error.Conflict;
//         }
//     }
//     None
// }
//
// #[test]
// pub fn TestRcReadCheckTS() {
//     let store = NewTestStore("TestRcReadCheckTS", "TestRcReadCheckTS", &testing::T::default());
//     MustPrewriteOptimistic(b("tk1"), b("tk1"), b("v1"), 1, 100, 0, &store);
//     MustCommit(b("tk1"), 1, 2, &store);
//     MustPrewriteOptimistic(b("tk2"), b("tk2"), b("v2"), 5, 100, 0, &store);
//     MustCommit(b("tk2"), 5, 6, &store);
//     MustPrewriteOptimistic(b("tk3"), b("tk3"), b("v3"), 10, 100, 0, &store);
//
//     let mut reqCtx = store.newReqCtx();
//     reqCtx.rpcCtx.ResolvedLocks = vec![];
//     reqCtx.rpcCtx.CommittedLocks = vec![];
//     reqCtx.rpcCtx.IsolationLevel = kvrpcpb::IsolationLevel_RCCheckTS;
//     assert_eq!(b("v1"), store.MvccStore.Get(reqCtx.clone(), b("tk1"), 3).unwrap());
//     let err = store.MvccStore.Get(reqCtx.clone(), b("tk2"), 3).unwrap_err();
//     let e = errors::Cause(err).downcast::<kverrors::ErrConflict>().unwrap();
//     assert_eq!(3_u64, e.StartTS);
//     assert_eq!(5_u64, e.ConflictTS);
//     assert_eq!(6_u64, e.ConflictCommitTS);
//     let err = store.MvccStore.Get(reqCtx.clone(), b("tk3"), 3).unwrap_err();
//     let e = errors::Cause(err).downcast::<kverrors::ErrConflict>().unwrap();
//     assert_eq!(3_u64, e.StartTS);
//     assert_eq!(10_u64, e.ConflictTS);
//
// Scan 与 reverse scan 都要从更近版本或锁上报告 RCCheckTS 冲突。
//     let mut scanReq = kvrpcpb::ScanRequest { Context: reqCtx.rpcCtx.clone(), StartKey: b("a"), Limit: 100, Version: 3, EndKey: b("z"), ..Default::default() };
//     let mut conflictErr = getConflictErr(store.MvccStore.Scan(reqCtx.clone(), &scanReq)).unwrap();
//     assert_eq!(3_u64, conflictErr.StartTs);
//     assert_eq!(5_u64, conflictErr.ConflictTs);
//     assert_eq!(6_u64, conflictErr.ConflictCommitTs);
//     scanReq.Version = 15;
//     conflictErr = getConflictErr(store.MvccStore.Scan(reqCtx.clone(), &scanReq)).unwrap();
//     assert_eq!(15_u64, conflictErr.StartTs);
//     assert_eq!(10_u64, conflictErr.ConflictTs);
//     scanReq.Version = 3;
//     scanReq.Reverse = true;
//     assert!(getConflictErr(store.MvccStore.Scan(reqCtx.clone(), &scanReq)).is_some());
//     scanReq.Version = 15;
//     assert!(getConflictErr(store.MvccStore.Scan(reqCtx, &scanReq)).is_some());
// }
// */
use crate::mvcc::{
    Action, KvPair, Mutation, MutationOp, MvccError, MvccStore, PessimisticLockRequest,
    PrewriteRequest, SafePoint,
};
use std::sync::Arc;

/// 构造 Put 类型的 Mutation。
fn put(key: &[u8], value: &[u8]) -> Mutation {
    Mutation {
        op: MutationOp::Put,
        key: key.to_vec(),
        value: value.to_vec(),
        is_pessimistic_lock: false,
    }
}

/// 构造以该键为主键的单键乐观 Prewrite 请求。
fn request(start_ts: u64, key: &[u8], value: &[u8]) -> PrewriteRequest {
    PrewriteRequest {
        mutations: vec![put(key, value)],
        primary_lock: key.to_vec(),
        start_ts,
        lock_ttl: 100,
        ..PrewriteRequest::default()
    }
}

#[test]
/// 乐观预写/提交后，读时间戳应遵循 MVCC 可见性（看不到未提交与未达 commit_ts 的版本）。
fn optimistic_prewrite_commit_and_snapshot_reads_follow_mvcc_visibility() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    store.prewrite(&request(10, b"k", b"v1")).unwrap();
    // 预写后、提交前：读应看到锁冲突。
    assert!(matches!(
        store.get(b"k", 11, &[]),
        Err(MvccError::KeyLocked { .. })
    ));
    store.commit(&[b"k".to_vec()], 10, 20).unwrap();
    // 读时间戳小于 commit_ts：快照不可见该版本。
    assert_eq!(None, store.get(b"k", 19, &[]).unwrap());
    assert_eq!(Some(b"v1".to_vec()), store.get(b"k", 20, &[]).unwrap());

    store.prewrite(&request(30, b"k", b"v2")).unwrap();
    store.commit(&[b"k".to_vec()], 30, 40).unwrap();
    assert_eq!(Some(b"v1".to_vec()), store.get(b"k", 39, &[]).unwrap());
    assert_eq!(Some(b"v2".to_vec()), store.get(b"k", 40, &[]).unwrap());
}

#[test]
/// 回滚可重复执行，且阻止迟到的 Commit。
fn rollback_is_idempotent_and_prevents_late_commit() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    store.prewrite(&request(5, b"k", b"value")).unwrap();
    // 连续两次 rollback 应幂等成功。
    store.rollback(&[b"k".to_vec()], 5).unwrap();
    store.rollback(&[b"k".to_vec()], 5).unwrap();
    assert!(matches!(
        store.commit(&[b"k".to_vec()], 5, 10),
        Err(MvccError::TxnNotFound { .. })
    ));
    assert_eq!(None, store.get(b"k", 20, &[]).unwrap());
}

#[test]
/// 一阶段提交（1PC）尊重 min_commit_ts，且不留下锁。
fn one_phase_commit_respects_min_commit_ts_without_leaving_a_lock() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    let mut req = request(10, b"one", b"pc");
    // 启用 1PC，并约束提交时间戳落在 [min, max] 内。
    req.try_one_pc = true;
    req.min_commit_ts = 15;
    req.max_commit_ts = 20;
    let result = store.prewrite(&req).unwrap();
    assert_eq!(15, result.one_pc_commit_ts);
    assert_eq!(Some(b"pc".to_vec()), store.get(b"one", 15, &[]).unwrap());
    assert!(store.mvcc_get_by_key(b"one").lock.is_none());
}

#[test]
/// check_txn_status 可推高 min_commit_ts；TTL 过期则回滚锁。
fn txn_status_pushes_min_commit_ts_and_expired_lock_rolls_back() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    store.prewrite(&request(10, b"k", b"v")).unwrap();
    // caller_start_ts 推高锁上的 min_commit_ts，避免过早提交。
    let pushed = store.check_txn_status(b"k", 10, 30, 10, false).unwrap();
    assert_eq!(Action::MinCommitTsPushed, pushed.action);
    assert_eq!(31, pushed.lock_info.unwrap().min_commit_ts);
    // current_ts 极大导致 TTL 过期，执行过期回滚。
    let expired = store
        .check_txn_status(b"k", 10, 30, u64::MAX, false)
        .unwrap();
    assert_eq!(Action::TtlExpireRollback, expired.action);
}

#[test]
/// Go 的 CheckNotExists 只做存在性校验，不应留下可阻塞后续读的锁。
fn check_not_exists_does_not_create_a_lock() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    let request = PrewriteRequest {
        mutations: vec![Mutation {
            op: MutationOp::CheckNotExists,
            key: b"absent".to_vec(),
            value: Vec::new(),
            is_pessimistic_lock: false,
        }],
        primary_lock: b"absent".to_vec(),
        start_ts: 10,
        lock_ttl: 100,
        ..PrewriteRequest::default()
    };

    store.prewrite(&request).unwrap();
    assert!(store.mvcc_get_by_key(b"absent").lock.is_none());
    assert_eq!(None, store.get(b"absent", 20, &[]).unwrap());
}

#[test]
/// Go 的主键读在 MAX_SYSTEM_TS 下允许读取自己的未提交 Put 锁。
fn primary_max_ts_read_can_bypass_its_write_lock() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    store.prewrite(&request(10, b"primary", b"value")).unwrap();

    assert_eq!(None, store.get(b"primary", u64::MAX, &[]).unwrap());
}

#[test]
/// 悲观锁不是写锁，普通 MVCC 读不应因它返回 KeyLocked。
fn pessimistic_lock_does_not_block_snapshot_read() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    store
        .pessimistic_lock(&PessimisticLockRequest {
            mutations: vec![Mutation {
                op: MutationOp::PessimisticLock,
                key: b"pessimistic".to_vec(),
                value: Vec::new(),
                is_pessimistic_lock: false,
            }],
            primary_lock: b"pessimistic".to_vec(),
            start_ts: 10,
            for_update_ts: 10,
            lock_ttl: 100,
            ..PessimisticLockRequest::default()
        })
        .unwrap();

    assert_eq!(None, store.get(b"pessimistic", 20, &[]).unwrap());
}

#[test]
/// 悲观锁批次应像 Go 的 WriteBatch 一样全量成功或全量失败。
fn pessimistic_lock_batch_does_not_leave_partial_locks_on_conflict() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    store
        .pessimistic_lock(&PessimisticLockRequest {
            mutations: vec![Mutation {
                op: MutationOp::PessimisticLock,
                key: b"blocked".to_vec(),
                value: Vec::new(),
                is_pessimistic_lock: false,
            }],
            primary_lock: b"blocked".to_vec(),
            start_ts: 20,
            for_update_ts: 20,
            lock_ttl: 100,
            ..PessimisticLockRequest::default()
        })
        .unwrap();

    let result = store.pessimistic_lock(&PessimisticLockRequest {
        mutations: vec![
            Mutation {
                op: MutationOp::PessimisticLock,
                key: b"available".to_vec(),
                value: Vec::new(),
                is_pessimistic_lock: false,
            },
            Mutation {
                op: MutationOp::PessimisticLock,
                key: b"blocked".to_vec(),
                value: Vec::new(),
                is_pessimistic_lock: false,
            },
        ],
        primary_lock: b"available".to_vec(),
        start_ts: 10,
        for_update_ts: 10,
        lock_ttl: 100,
        ..PessimisticLockRequest::default()
    });

    assert!(matches!(result, Err(MvccError::KeyLocked { .. })));
    assert!(store.mvcc_get_by_key(b"available").lock.is_none());
}

#[test]
/// CheckTxnStatus 不应推进异步提交锁的 min_commit_ts，除非强制同步。
fn async_commit_status_does_not_push_min_commit_ts() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    let mut request = request(10, b"async", b"value");
    request.use_async_commit = true;
    request.secondaries = vec![b"secondary".to_vec()];
    store.prewrite(&request).unwrap();

    let status = store.check_txn_status(b"async", 10, 30, 20, false).unwrap();
    assert_eq!(Action::NoAction, status.action);
    assert_eq!(11, status.lock_info.unwrap().min_commit_ts);
}

#[test]
/// CheckTxnStatus 必须拒绝使用错误主键查询事务状态。
fn txn_status_rejects_primary_key_mismatch() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    let mut request = request(10, b"primary", b"value");
    request.mutations.push(put(b"secondary", b"value-2"));
    store.prewrite(&request).unwrap();

    assert!(matches!(
        store.check_txn_status(b"secondary", 10, 30, 20, false),
        Err(MvccError::PrimaryMismatch { .. })
    ));
}

#[test]
/// 长值走 defaults 后，提交读到原值；Delete 提交后应遮蔽旧版本。
fn long_value_and_delete_preserve_mvcc_visibility() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    let long = vec![b'x'; 128];
    store.prewrite(&request(10, b"long", &long)).unwrap();
    store.commit(&[b"long".to_vec()], 10, 20).unwrap();
    assert_eq!(Some(long), store.get(b"long", 20, &[]).unwrap());

    let delete = PrewriteRequest {
        mutations: vec![Mutation {
            op: MutationOp::Delete,
            key: b"long".to_vec(),
            value: Vec::new(),
            is_pessimistic_lock: false,
        }],
        primary_lock: b"long".to_vec(),
        start_ts: 30,
        lock_ttl: 100,
        ..PrewriteRequest::default()
    };
    store.prewrite(&delete).unwrap();
    store.commit(&[b"long".to_vec()], 30, 40).unwrap();
    assert_eq!(None, store.get(b"long", 40, &[]).unwrap());
}

#[test]
/// 悲观锁心跳只增长 TTL，Rollback 需匹配 start/for-update 时间戳。
fn pessimistic_heartbeat_and_rollback_require_matching_timestamps() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    let request = PessimisticLockRequest {
        mutations: vec![Mutation {
            op: MutationOp::PessimisticLock,
            key: b"p".to_vec(),
            value: Vec::new(),
            is_pessimistic_lock: false,
        }],
        primary_lock: b"p".to_vec(),
        start_ts: 10,
        for_update_ts: 12,
        lock_ttl: 20,
        ..PessimisticLockRequest::default()
    };
    store.pessimistic_lock(&request).unwrap();
    assert_eq!(30, store.txn_heartbeat(b"p", 10, 30).unwrap());
    store.pessimistic_rollback(&[b"p".to_vec()], 10, 11);
    assert!(store.mvcc_get_by_key(b"p").lock.is_some());
    store.pessimistic_rollback(&[b"p".to_vec()], 10, 12);
    assert!(store.mvcc_get_by_key(b"p").lock.is_none());
}

#[test]
/// Scan 应覆盖正向、反向、limit、key-only 与已提交版本可见性。
fn scan_supports_direction_limit_and_key_only() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    for (index, key) in [b"a".as_slice(), b"b", b"c"].into_iter().enumerate() {
        store
            .prewrite(&request(index as u64 + 1, key, key))
            .unwrap();
        store
            .commit(&[key.to_vec()], index as u64 + 1, index as u64 + 10)
            .unwrap();
    }
    let forward = store.scan(b"a", b"", 20, 2, false, true, &[]);
    assert_eq!(
        vec![b"a".to_vec(), b"b".to_vec()],
        forward.iter().map(|p| p.key.clone()).collect::<Vec<_>>()
    );
    assert!(forward.iter().all(|pair| pair.value.is_empty()));
    let reverse = store.scan(b"a", b"", 20, 2, true, false, &[]);
    assert_eq!(
        vec![b"c".to_vec(), b"b".to_vec()],
        reverse.iter().map(|p| p.key.clone()).collect::<Vec<_>>()
    );
}

#[test]
/// Scan 在 safe point 之前必须像点查一样返回 GC-too-early 错误。
fn scan_rejects_versions_before_safe_point() {
    let store = MvccStore::new(Arc::new(SafePoint::new(50)));
    let result = store.scan(b"a", b"z", 10, 10, false, false, &[]);
    assert!(matches!(
        result.as_slice(),
        [KvPair {
            error: Some(MvccError::GcTooEarly { .. }),
            ..
        }]
    ));
}

#[test]
/// GC 保留 safe point 前最后一个可见锚点，并清理更早历史。
fn gc_keeps_an_anchor_version_and_drops_older_history() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    for (start, commit, value) in [(1, 2, b"v1"), (3, 4, b"v2"), (5, 6, b"v3")] {
        store.prewrite(&request(start, b"k", value)).unwrap();
        store.commit(&[b"k".to_vec()], start, commit).unwrap();
    }
    store.update_safe_point(5);
    store.gc();
    let info = store.mvcc_get_by_key(b"k");
    assert_eq!(2, info.writes.len());
    assert_eq!(Some(b"v2".to_vec()), store.get(b"k", 5, &[]).unwrap());
    assert_eq!(Some(b"v3".to_vec()), store.get(b"k", 6, &[]).unwrap());
}

#[test]
/// ResolveLock 应按 commit_ts 选择提交或回滚，并清理对应锁。
fn resolve_lock_commits_or_rolls_back_all_matching_locks() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    let mut req = request(10, b"a", b"a-value");
    req.mutations.push(put(b"b", b"b-value"));
    store.prewrite(&req).unwrap();
    store.resolve_lock(10, 20).unwrap();
    assert_eq!(Some(b"a-value".to_vec()), store.get(b"a", 20, &[]).unwrap());
    assert_eq!(Some(b"b-value".to_vec()), store.get(b"b", 20, &[]).unwrap());

    store.prewrite(&request(30, b"c", b"c-value")).unwrap();
    store.resolve_lock(30, 0).unwrap();
    assert_eq!(None, store.get(b"c", 40, &[]).unwrap());
}

#[test]
/// Go 的 Commit 仅在整批锁都校验成功后写入；后序键失败不能提交前序键。
fn commit_batch_is_atomic_when_a_later_key_fails_validation() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    store.prewrite(&request(10, b"a", b"a-value")).unwrap();
    let mut later = request(10, b"b", b"b-value");
    later.min_commit_ts = 30;
    store.prewrite(&later).unwrap();

    assert!(matches!(
        store.commit(&[b"a".to_vec(), b"b".to_vec()], 10, 20),
        Err(MvccError::CommitTsExpired { .. })
    ));
    assert!(store.mvcc_get_by_key(b"a").lock.is_some());
    assert!(store.mvcc_get_by_key(b"b").lock.is_some());
    assert_eq!(None, store.get(b"a", 20, &[10]).unwrap());
}

#[test]
/// Go 的 Rollback 在发现任一键已提交时不会写入前序键的 rollback 记录。
fn rollback_batch_is_atomic_when_a_later_key_is_committed() {
    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    store
        .prewrite(&request(10, b"committed", b"value"))
        .unwrap();
    store.commit(&[b"committed".to_vec()], 10, 20).unwrap();

    assert!(matches!(
        store.rollback(&[b"absent".to_vec(), b"committed".to_vec()], 10),
        Err(MvccError::AlreadyCommitted(20))
    ));
    assert!(store.mvcc_get_by_key(b"absent").writes.is_empty());
}
