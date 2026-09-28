// Copyright 2026 AsterSQL.
use super::*;

fn cache() -> cacheDB {
    cacheDB {
        memTables: RwLock::new(HashMap::new()),
    }
}

#[test]
fn rejected_write_still_creates_table() {
    let cache = cache();
    for (tid, key, value, message) in [
        (
            1,
            Key(vec![0; 65536]),
            vec![],
            "The key is larger than 65535",
        ),
        (
            2,
            Key(vec![0]),
            vec![0; 102376],
            "The entry size is larger than 1/1024 of cache size",
        ),
    ] {
        assert_eq!(
            cache.set(tid, &key, &value).unwrap_err().to_string(),
            message
        );
        assert!(cache.memTables.read().unwrap().contains_key(&tid));
        assert_eq!(cache.get(tid, &key), None);
        cache.Delete(tid);
        assert!(!cache.memTables.read().unwrap().contains_key(&tid));
    }
}

#[test]
fn cache_boundaries_copy_isolation_and_delete() {
    let cache = cache();
    for (key, value) in [(Key(vec![]), vec![]), (Key(vec![1; 65535]), vec![2; 36841])] {
        cache.set(1, &key, &value).unwrap();
        assert_eq!(
            cache.get(1, &key),
            if value.is_empty() {
                None
            } else {
                Some(value.clone())
            }
        );
        assert_eq!(cache.get(2, &key), None);
        let mut copy = cache.get(1, &key).unwrap_or_default();
        copy.push(9);
        assert_eq!(
            cache.get(1, &key),
            if value.is_empty() { None } else { Some(value) }
        );
        cache.set(1, &key, &[3]).unwrap();
        assert_eq!(cache.get(1, &key), Some(vec![3]));
        cache.Delete(1);
        cache.Delete(1);
        assert_eq!(cache.get(1, &key), None);
    }
}

struct CountingSnapshot {
    calls: std::sync::atomic::AtomicUsize,
    value: Option<Vec<u8>>,
}
impl Getter for CountingSnapshot {
    fn Get(&self, _: &Context, _: Key, _: &[GetOption]) -> Result<ValueEntry, Error> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.value
            .clone()
            .map(|v| NewValueEntry(v, 0))
            .ok_or_else(|| errors::New("snapshot failure"))
    }
}
impl Retriever for CountingSnapshot {
    fn Iter(&self, _: Key, _: Option<Key>) -> Result<Box<dyn Iterator>, Error> {
        panic!("unexpected Iter")
    }
    fn IterReverse(&self, _: Option<Key>, _: Option<Key>) -> Result<Box<dyn Iterator>, Error> {
        panic!("unexpected IterReverse")
    }
}
impl Snapshot for CountingSnapshot {
    fn BatchGet(
        &self,
        _: &Context,
        _: &[Key],
        _: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, Error> {
        panic!("unexpected BatchGet")
    }
    fn SetOption(&mut self, _: i32, _: Option<Box<dyn std::any::Any>>) {
        panic!("unexpected SetOption")
    }
}
#[test]
fn union_get_empty_values_and_errors_are_not_hits() {
    for value in [Some(vec![]), Some(vec![7]), None] {
        let snapshot = CountingSnapshot {
            calls: Default::default(),
            value: value.clone(),
        };
        let cache = NewCacheDB();
        let key = Key(vec![1]);
        for _ in 0..2 {
            let result = cache.UnionGet(&Context::todo(), 1, &snapshot, &key);
            match &value {
                Some(value) => assert_eq!(result.unwrap(), *value),
                None => assert_eq!(result.unwrap_err().to_string(), "snapshot failure"),
            }
        }
        let expected = if value.as_ref().is_some_and(|v| !v.is_empty()) {
            1
        } else {
            2
        };
        assert_eq!(
            snapshot.calls.load(std::sync::atomic::Ordering::SeqCst),
            expected
        );
        cache.Delete(1);
        let _ = cache.UnionGet(&Context::todo(), 1, &snapshot, &key);
        assert_eq!(
            snapshot.calls.load(std::sync::atomic::Ordering::SeqCst),
            expected + 1
        );
    }
}

#[test]
fn manager_can_be_shared_between_threads() {
    fn require_shared<T: Send + Sync + ?Sized>(_: &T) {}
    let cache = NewCacheDB();
    require_shared(cache.as_ref());
    std::thread::scope(|scope| {
        for tid in 0..4 {
            let cache = &cache;
            scope.spawn(move || cache.Delete(tid));
        }
    });
}

// Expected misses come from freecache v1.2.1 with the same 100 MiB capacity.
// These xxHash64 keys all select segment zero: five 100 KB entries exceed
// that segment's 409600 bytes even though the global cache is almost empty.
#[test]
fn segment_pressure_evicts_and_union_get_refetches() {
    let cache = cache();
    let keys = [
        "collision-545",
        "collision-602",
        "collision-863",
        "collision-1361",
        "collision-2130",
    ];
    for key in keys {
        cache
            .set(1, &Key(key.as_bytes().to_vec()), &vec![1; 100000])
            .unwrap();
    }
    let oldest = Key(keys[0].as_bytes().to_vec());
    assert_eq!(cache.get(1, &oldest), None);
    for key in &keys[1..] {
        assert_eq!(
            cache.get(1, &Key(key.as_bytes().to_vec())).unwrap().len(),
            100000
        );
    }
    let snapshot = CountingSnapshot {
        calls: Default::default(),
        value: Some(vec![9]),
    };
    assert_eq!(
        cache
            .UnionGet(&Context::todo(), 1, &snapshot, &oldest)
            .unwrap(),
        vec![9]
    );
    assert_eq!(snapshot.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

// Generated by running the identical operation stream against freecache v1.2.1
// segment.set/get, with a custom Timer (as supported by its own Go tests).
// Digest includes every read, ring byte, live slot pointer, and accounting
// field after every operation, not just final hit/miss counts. Modes cover
// frozen time, changing time + slot expansion, full hash collisions, and a
// backwards wall clock (Go get uses uint32 wrapping timestamp subtraction).
#[test]
fn freecache_v1_2_1_differential_ring_traces() {
    fn mix(digest: &mut u64, value: u64) {
        for byte in value.to_le_bytes() {
            *digest = (*digest ^ u64::from(byte)).wrapping_mul(1099511628211);
        }
    }
    let expected = [
        16521662391554494459,
        171116731863848597,
        11580398643794799964,
        16680542174930994888,
    ];
    for (mode, expected) in expected.into_iter().enumerate() {
        let mut segment = CacheSegment::new(2048);
        let mut now = 100u32;
        let mut digest = 14695981039346656037u64;
        let mut state = 0x12345678u64;
        for step in 0..6000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let key = (state % 48).to_le_bytes();
            let mut hash = xxhash_rust::xxh64::xxh64(&key, 0) & !255;
            if mode == 1 {
                hash &= !(255 << 8);
            }
            if mode == 2 {
                hash = 0;
            }
            if mode != 0 && step % 13 == 0 {
                now = now.wrapping_add(1);
            }
            if mode == 3 && step == 3000 {
                now = now.wrapping_sub(1000);
            }
            if state % 5 < 3 {
                let length = if step % 29 == 0 {
                    0
                } else {
                    ((state >> 16) % 480) as usize
                };
                let value: Vec<_> = (0..length).map(|j| (step + j) as u8).collect();
                segment.set(&key, &value, hash, now);
                mix(&mut digest, 1);
            } else {
                match segment.get(&key, hash, now) {
                    None => mix(&mut digest, 2),
                    Some(value) => {
                        mix(&mut digest, 3);
                        mix(&mut digest, value.len() as u64);
                        for byte in value {
                            mix(&mut digest, byte as u64);
                        }
                    }
                }
            }
            for value in [
                segment.end,
                segment.vacuum as u64,
                segment.total_time as u64,
                segment.total_count as u64,
                segment.slot_cap as u64,
            ] {
                mix(&mut digest, value);
            }
            for slot in 0..256 {
                mix(&mut digest, segment.slot_lens[slot] as u64);
                let start = slot * segment.slot_cap;
                for ptr in &segment.slots[start..start + segment.slot_lens[slot]] {
                    mix(&mut digest, ptr.offset);
                    mix(&mut digest, ptr.hash16 as u64);
                    mix(&mut digest, ptr.key_len as u64);
                }
            }
            for byte in &segment.data {
                mix(&mut digest, *byte as u64);
            }
        }
        assert_eq!(digest, expected, "Go freecache differential mode {mode}");
    }
}

// Optional reproducibility check; ordinary cargo test validates the committed
// oracle digests without requiring a Go installation or network access.
#[test]
#[ignore = "requires Go and the cached freecache v1.2.1/xxhash v2.3.0 modules"]
fn regenerate_freecache_go_oracle() {
    use std::{fs, process::Command};
    let root = std::env::temp_dir().join(format!(
        "astersql-cachedb-oracle-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let manifest = "module cachedb-oracle\ngo 1.23\nrequire (\n github.com/coocood/freecache v1.2.1\n github.com/cespare/xxhash/v2 v2.3.0\n)\n";
    fs::write(root.join("go.mod"), manifest).unwrap();
    let run = |cwd: &std::path::Path, args: &[&str]| {
        let output = Command::new("go")
            .args(args)
            .current_dir(cwd)
            .env("GOWORK", "off")
            .env("GOPROXY", "off")
            .env("GOSUMDB", "off")
            .output()
            .expect("Go must be installed for oracle regeneration");
        assert!(
            output.status.success(),
            "Go oracle: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(
        &root,
        &[
            "mod",
            "download",
            "github.com/coocood/freecache",
            "github.com/cespare/xxhash/v2",
        ],
    );
    let source = run(
        &root,
        &[
            "list",
            "-mod=mod",
            "-m",
            "-f",
            "{{.Dir}}",
            "github.com/coocood/freecache",
        ],
    );
    let local = root.join("freecache");
    fs::create_dir(&local).unwrap();
    for entry in fs::read_dir(source.trim()).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name.ends_with(".go") && !name.ends_with("_test.go")) || name == "LICENSE" {
            fs::copy(entry.path(), local.join(name)).unwrap();
        }
    }
    fs::write(local.join("cachedb_parity_test.go"), GO_FREECACHE_ORACLE).unwrap();
    let output = run(
        &root,
        &[
            "test",
            "-mod=mod",
            "./freecache",
            "-run",
            "TestCachedbParity",
            "-v",
        ],
    );
    for expected in [
        "MODE 0 DIGEST 16521662391554494459",
        "MODE 1 DIGEST 171116731863848597",
        "MODE 2 DIGEST 11580398643794799964",
        "MODE 3 DIGEST 16680542174930994888",
    ] {
        assert!(output.contains(expected), "Go output changed: {output}");
    }
    fs::remove_dir_all(root).unwrap();
}

const GO_FREECACHE_ORACLE: &str = r#"package freecache
import("fmt"; "testing"; "encoding/binary")
type parityTimer struct { now uint32 }
func(t *parityTimer) Now() uint32 {return t.now}
func TestCachedbParity(t *testing.T) {
 for mode:=0;mode<4;mode++ {
  timer:=&parityTimer{100}; seg:=newSegment(2048,0,timer); digest:=uint64(14695981039346656037); state:=uint64(0x12345678)
  mix:=func(x uint64) {for i:=0;i<8;i++ {digest^=uint64(byte(x));digest*=1099511628211;x>>=8}}
  for step:=0;step<6000;step++ {
   state^=state<<13;state^=state>>7;state^=state<<17
   id:=state%48; key:=make([]byte,8);binary.LittleEndian.PutUint64(key,id)
   hash:=hashFunc(key)&^uint64(255);if mode==1 {hash &^=uint64(255)<<8};if mode==2 {hash=0}
   if mode!=0 && step%13==0 {timer.now++}
   if mode==3 && step==3000 {timer.now-=1000}
   if state%5<3 {
    length:=int((state>>16)%480); if step%29==0 {length=0}
    value:=make([]byte,length);for j:=range value {value[j]=byte(step+j)}
    if err:=seg.set(key,value,hash,0);err!=nil {t.Fatal(err)}
    mix(1)
   } else {
    value,_,err:=seg.get(key,nil,hash,false);if err!=nil {mix(2)} else {mix(3);mix(uint64(len(value)));for _,b:=range value {mix(uint64(b))}}
   }
   mix(uint64(seg.rb.End()));mix(uint64(seg.vacuumLen));mix(uint64(seg.totalTime));mix(uint64(seg.totalCount));mix(uint64(seg.slotCap))
   for i:=0;i<256;i++ {mix(uint64(seg.slotLens[i]));for _,p:=range seg.getSlot(uint8(i)) {mix(uint64(p.offset));mix(uint64(p.hash16));mix(uint64(p.keyLen))}}
   for _,b:=range seg.rb.data {mix(uint64(b))}
  }
  fmt.Printf("MODE %d DIGEST %d\n",mode,digest)
 }
}
"#;

#[test]
fn concurrent_reads_writes_and_table_delete_preserve_values() {
    let cache = NewCacheDB();
    std::thread::scope(|scope| {
        for worker in 0..4 {
            let cache = &cache;
            scope.spawn(move || {
                let snapshot = CountingSnapshot {
                    calls: Default::default(),
                    value: Some(vec![worker]),
                };
                let key = Key(vec![worker]);
                for step in 0..20 {
                    if step == 10 {
                        cache.Delete(1);
                    }
                    assert_eq!(
                        cache
                            .UnionGet(&Context::todo(), 1, &snapshot, &key)
                            .unwrap(),
                        vec![worker]
                    );
                }
            });
        }
    });
}

#[test]
fn overwrite_retains_capacity_and_growth_leaves_tombstone() {
    let mut segment = CacheSegment::new(2048);
    let key = b"k";
    let hash = xxhash_rust::xxh64::xxh64(key, 0);
    segment.set(key, &[1; 100], hash, 1);
    assert_eq!(segment.end, 125);
    segment.set(key, &[2], hash, 2);
    assert_eq!(segment.end, 125);
    assert_eq!(segment.header(0).value_cap, 100);
    segment.set(key, &[3; 101], hash, 3);
    assert!(segment.header(0).deleted);
    assert_eq!(segment.header(125).value_cap, 200);
    assert_eq!(segment.end, 350);
    assert_eq!(segment.total_count, 2);
    // Deleted entries retain their previous access time until evacuated.
    assert_eq!(segment.total_time, 5);
    assert_eq!(segment.get(key, hash, 4), Some(vec![3; 101]));
    segment.set(key, &[4; 480], hash, 5);
    assert_eq!(segment.header(350).value_cap, 487);
    assert_eq!(segment.end, 862);
}
