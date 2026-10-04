// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

//! Combined global TopN and histogram merge, from histogram.go's two passes.
use crate::{Histogram, NewHistogram, NewTopN, TopN, TopNMeta, topNMetaToDatum};
use astersql_errors::SharedError;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

type MergeResult<T> = Result<T, SharedError>;
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct BucketRef {
    hist: u16,
    bucket: u16,
}
#[derive(Clone)]
struct Candidate {
    encoded: Vec<u8>,
    count: u64,
    repeat: i64,
}

struct MergeContext<'a> {
    sc: &'a stmtctx::StatementContext,
    location: chrono_tz::Tz,
    is_index: bool,
}
impl MergeContext<'_> {
    fn compare(&self, a: &types::Datum, b: &types::Datum) -> MergeResult<i32> {
        a.Compare(self.sc.TypeCtx(), b, collate::GetBinaryCollator().as_ref())
            .map_err(|error| astersql_errors::New(error.to_string()))
    }
    fn encode(&self, d: &types::Datum) -> MergeResult<Vec<u8>> {
        if self.is_index {
            Ok(d.GetBytes().to_vec())
        } else {
            codec::EncodeKey(self.location, Vec::new(), vec![d.clone()])
        }
    }
    fn decode(&self, encoded: &[u8], ft: &types::FieldType) -> MergeResult<types::Datum> {
        topNMetaToDatum(
            &TopNMeta {
                Encoded: encoded.to_vec(),
                Count: 0,
            },
            ft,
            self.is_index,
            self.location,
        )
    }
}
fn down<T>(
    heap: &mut [T],
    mut i: usize,
    mut less: impl FnMut(&T, &T) -> MergeResult<bool>,
) -> MergeResult<()> {
    loop {
        let left = i * 2 + 1;
        if left >= heap.len() {
            return Ok(());
        }
        let right = left + 1;
        let j = if right < heap.len() && less(&heap[right], &heap[left])? {
            right
        } else {
            left
        };
        if !less(&heap[j], &heap[i])? {
            return Ok(());
        }
        heap.swap(i, j);
        i = j;
    }
}
fn up<T>(
    heap: &mut [T],
    mut j: usize,
    mut less: impl FnMut(&T, &T) -> MergeResult<bool>,
) -> MergeResult<()> {
    while j > 0 {
        let i = (j - 1) / 2;
        if !less(&heap[j], &heap[i])? {
            break;
        }
        heap.swap(i, j);
        j = i;
    }
    Ok(())
}

struct BucketCursor<'a> {
    heap: Vec<BucketRef>,
    hists: &'a [Option<Histogram>],
    context: &'a MergeContext<'a>,
    refs: Vec<BucketRef>,
}
impl BucketCursor<'_> {
    fn first(&self, hi: usize, start: usize) -> Option<BucketRef> {
        let h = self.hists[hi].as_ref()?;
        (start..h.Len())
            .find(|&bi| h.BucketCount(bi) > 0)
            .map(|bi| BucketRef {
                hist: hi as u16,
                bucket: bi as u16,
            })
    }
    fn upper(&self, r: BucketRef) -> &types::Datum {
        self.hists[r.hist as usize]
            .as_ref()
            .unwrap()
            .GetUpper(r.bucket as usize)
    }
    fn advance(&mut self) -> MergeResult<BucketRef> {
        let r = self.heap.swap_remove(0);
        let ctx = self.context;
        let hists = self.hists;
        let less = |a: &BucketRef, b: &BucketRef| {
            Ok(ctx.compare(
                hists[a.hist as usize]
                    .as_ref()
                    .unwrap()
                    .GetUpper(a.bucket as usize),
                hists[b.hist as usize]
                    .as_ref()
                    .unwrap()
                    .GetUpper(b.bucket as usize),
            )? < 0)
        };
        down(&mut self.heap, 0, less)?;
        self.refs.push(r);
        if let Some(next) = self.first(r.hist as usize, r.bucket as usize + 1) {
            self.heap.push(next);
            let end = self.heap.len() - 1;
            up(&mut self.heap, end, less)?;
        }
        Ok(r)
    }
    fn group(&mut self, need_encoded: bool) -> MergeResult<(Vec<u8>, i64)> {
        let first = self.advance()?;
        let upper = self.upper(first).clone();
        let mut repeat =
            self.hists[first.hist as usize].as_ref().unwrap().Buckets[first.bucket as usize].Repeat;
        while let Some(&r) = self.heap.first() {
            if self.context.compare(self.upper(r), &upper)? != 0 {
                break;
            }
            let r = self.advance()?;
            repeat +=
                self.hists[r.hist as usize].as_ref().unwrap().Buckets[r.bucket as usize].Repeat;
        }
        Ok((
            if need_encoded {
                self.context.encode(&upper)?
            } else {
                Vec::new()
            },
            repeat,
        ))
    }
}

struct MergeRefs<'a> {
    refs: Vec<BucketRef>,
    hists: Vec<Option<Cow<'a, Histogram>>>,
    promoted: HashSet<Vec<u8>>,
    remaining: HashMap<BucketRef, i64>,
    effective: HashMap<BucketRef, types::Datum>,
    context: &'a MergeContext<'a>,
}
impl MergeRefs<'_> {
    fn hist(&self, r: BucketRef) -> &Histogram {
        self.hists[r.hist as usize].as_ref().unwrap()
    }
    fn upper(&self, r: BucketRef) -> &types::Datum {
        self.hist(r).GetUpper(r.bucket as usize)
    }
    fn lower(&self, r: BucketRef) -> &types::Datum {
        self.hist(r).GetLower(r.bucket as usize)
    }
    fn effective(&self, r: BucketRef) -> &types::Datum {
        self.effective.get(&r).unwrap_or_else(|| self.upper(r))
    }
    fn mass(&self, r: BucketRef) -> MergeResult<(i64, i64, bool)> {
        if let Some(&remaining) = self.remaining.get(&r) {
            return Ok((remaining, 0, false));
        }
        let hist = self.hist(r);
        let bi = r.bucket as usize;
        let mut mass = hist.BucketCount(bi);
        let mut repeat = hist.Buckets[bi].Repeat;
        if self.promoted.contains(&self.context.encode(self.upper(r))?) {
            mass -= repeat;
            repeat = 0;
        }
        Ok((mass, repeat, true))
    }
    fn merge_virtual(&mut self, entries: &[Candidate], first: &Histogram) -> MergeResult<()> {
        let size = u16::MAX as usize;
        let chunks = entries.len().div_ceil(size);
        if self.hists.len() + chunks > size {
            return Err(astersql_errors::New(format!(
                "MergePartTopNAndHistToGlobal: too many virtual histograms ({} partitions + {} virtual chunks > {})",
                self.hists.len(),
                chunks,
                size
            )));
        }
        let base = self.hists.len();
        for chunk in entries.chunks(size) {
            let mut h = NewHistogram(first.ID, 0, 0, 0, &first.Tp, chunk.len(), 0);
            let mut cum = 0;
            for entry in chunk {
                let d = self.context.decode(&entry.encoded, &first.Tp)?;
                cum += entry.count as i64;
                h.AppendBucketWithNDV(&d, &d, cum, entry.count as i64, 0);
            }
            self.hists.push(Some(Cow::Owned(h)));
        }
        let virtual_ref = |vi: usize| BucketRef {
            hist: (base + vi / size) as u16,
            bucket: (vi % size) as u16,
        };
        let mut unified = Vec::with_capacity(self.refs.len() + entries.len());
        let (mut si, mut vi) = (0, 0);
        while si < self.refs.len() && vi < entries.len() {
            if self
                .context
                .compare(self.upper(self.refs[si]), self.upper(virtual_ref(vi)))?
                <= 0
            {
                unified.push(self.refs[si]);
                si += 1;
            } else {
                unified.push(virtual_ref(vi));
                vi += 1;
            }
        }
        unified.extend_from_slice(&self.refs[si..]);
        unified.extend((vi..entries.len()).map(virtual_ref));
        self.refs = unified;
        Ok(())
    }
    fn build(
        &mut self,
        killer: &sqlkiller::sqlkiller::SQLKiller,
        total: i64,
        null: i64,
        size: i64,
        buckets: i64,
        first: &Histogram,
    ) -> MergeResult<Histogram> {
        let mut output = NewHistogram(
            first.ID,
            0,
            null,
            first.LastUpdateVersion,
            &first.Tp,
            buckets as usize,
            size,
        );
        if total <= 0 || self.refs.is_empty() {
            return Ok(output);
        }
        let target = (total / buckets).max(1);
        let mut rtl = Vec::with_capacity(buckets as usize);
        let (mut sum, mut previous, mut bucket_count) = (0, 0, 1);
        let (mut lower, mut upper): (Option<types::Datum>, Option<types::Datum>) = (None, None);
        let mut repeat_sum = 0;
        let mut i = self.refs.len() as isize - 1;
        let mut iteration = 0;
        while i >= 0 {
            if iteration & 1023 == 0 {
                killer.HandleSignal()?;
            }
            iteration += 1;
            let r = self.refs[i as usize];
            let (mass, repeat, fresh) = self.mass(r)?;
            if mass <= 0 {
                i -= 1;
                continue;
            }
            let effective = self.effective(r).clone();
            if upper.is_none() {
                upper = Some(effective.clone());
            }
            let lo = self.lower(r).clone();
            let extend = match lower.as_ref() {
                None => true,
                Some(l) => self.context.compare(&lo, l)? < 0,
            };
            if extend {
                lower = Some(lo);
            }
            sum += mass;
            if fresh
                && repeat > 0
                && self.context.compare(&effective, upper.as_ref().unwrap())? == 0
            {
                repeat_sum += repeat;
            }
            if sum < total * bucket_count / buckets
                || (sum - previous) * 5 < target * 4
                || rtl.len() as i64 >= buckets - 1
            {
                i -= 1;
                continue;
            }
            while i > 0 {
                let ahead = self.refs[i as usize - 1];
                let (mass, repeat, fresh) = self.mass(ahead)?;
                if mass <= 0 {
                    i -= 1;
                    continue;
                }
                if self
                    .context
                    .compare(self.effective(ahead), upper.as_ref().unwrap())?
                    != 0
                {
                    break;
                }
                i -= 1;
                sum += mass;
                if fresh && repeat > 0 {
                    repeat_sum += repeat;
                }
                let lo = self.lower(ahead).clone();
                if self.context.compare(&lo, lower.as_ref().unwrap())? < 0 {
                    lower = Some(lo);
                }
            }
            let cut = lower.as_ref().unwrap().clone();
            let mut j = i - 1;
            while j >= 0 {
                let scan = self.refs[j as usize];
                let (mass, repeat, _) = self.mass(scan)?;
                j -= 1;
                if mass <= 0 {
                    continue;
                }
                let up = self.effective(scan).clone();
                if self.context.compare(&up, &cut)? <= 0 {
                    break;
                }
                let lo = self.lower(scan).clone();
                if self.context.compare(&lo, &cut)? >= 0 || mass - repeat <= 0 {
                    sum += mass;
                    self.remaining.insert(scan, 0);
                    continue;
                }
                let right = ((mass - repeat) as f64
                    * (1.0 - crate::calcFraction4Datums(&lo, &up, &cut)))
                    as i64
                    + repeat;
                sum += right;
                self.remaining.insert(scan, mass - right);
                self.effective.insert(scan, cut.clone());
            }
            rtl.push((
                lower.take().unwrap(),
                upper.take().unwrap(),
                sum - previous,
                repeat_sum,
            ));
            bucket_count += 1;
            previous = sum;
            repeat_sum = 0;
            i -= 1;
        }
        if let (Some(lo), Some(up)) = (lower, upper) {
            if sum > previous {
                rtl.push((lo, up, sum - previous, repeat_sum));
            }
        }
        let mut cum = 0;
        for (lo, up, mass, repeat) in rtl.into_iter().rev() {
            cum += mass;
            output.AppendBucketWithNDV(&lo, &up, cum, repeat, 0);
        }
        Ok(output)
    }
}

/// Merge both partition streams without mutating their snapshots. SQL killer signals
/// are checked every 1024 iterations in both passes, as in Go's SQLKiller path.
pub fn MergePartTopNAndHistToGlobal(
    sc: &stmtctx::StatementContext,
    killer: &sqlkiller::sqlkiller::SQLKiller,
    top_ns: &[Option<TopN>],
    hists: &[Option<Histogram>],
    num_top_n: u32,
    expected_buckets: i64,
    is_index: bool,
) -> MergeResult<(Option<TopN>, Histogram)> {
    assert!(expected_buckets > 0, "expBucketNumber must be positive");
    if hists.len() > u16::MAX as usize {
        return Err(astersql_errors::New(format!(
            "MergePartTopNAndHistToGlobal: too many partition histograms ({} > {})",
            hists.len(),
            u16::MAX
        )));
    }
    for h in hists.iter().flatten() {
        if h.Len() > u16::MAX as usize {
            return Err(astersql_errors::New(format!(
                "MergePartTopNAndHistToGlobal: partition histogram has too many buckets ({} > {})",
                h.Len(),
                u16::MAX
            )));
        }
    }
    let first = hists.iter().flatten().next().ok_or_else(|| {
        astersql_errors::New("MergePartTopNAndHistToGlobal: no partition histograms provided")
    })?;
    let ctx = MergeContext {
        sc,
        location: sc.TimeZone(),
        is_index,
    };
    let mut entries = top_ns
        .iter()
        .flatten()
        .flat_map(|t| t.TopN.iter())
        .map(|e| Candidate {
            encoded: e.Encoded.clone(),
            count: e.Count,
            repeat: 0,
        })
        .collect::<Vec<_>>();
    let byte_ordered = is_index
        || !matches!(
            first.Tp.GetType(),
            types::mysql::TypeEnum | types::mysql::TypeSet | types::mysql::TypeBit
        );
    if byte_ordered {
        entries.sort_by(|a, b| a.encoded.cmp(&b.encoded));
    } else {
        let mut datums = HashMap::new();
        for e in &entries {
            if !datums.contains_key(&e.encoded) {
                datums.insert(e.encoded.clone(), ctx.decode(&e.encoded, &first.Tp)?);
            }
        }
        let mut error = None;
        entries.sort_by(
            |a, b| match ctx.compare(&datums[&a.encoded], &datums[&b.encoded]) {
                Ok(c) => c.cmp(&0).then_with(|| a.encoded.cmp(&b.encoded)),
                Err(e) => {
                    error = Some(e);
                    std::cmp::Ordering::Equal
                }
            },
        );
        if let Some(e) = error {
            return Err(e);
        }
    }
    let mut compact: Vec<Candidate> = Vec::with_capacity(entries.len());
    for e in entries {
        if let Some(last) = compact.last_mut().filter(|last| last.encoded == e.encoded) {
            last.count += e.count;
        } else {
            compact.push(e);
        }
    }
    let total = hists
        .iter()
        .flatten()
        .map(|h| h.Buckets.last().map_or(0, |bucket| bucket.Count))
        .sum::<i64>();
    let null = hists.iter().flatten().map(|h| h.NullCount).sum();
    let size = hists.iter().flatten().map(|h| h.TotColSize).sum();
    let mut cur = BucketCursor {
        heap: Vec::with_capacity(hists.len()),
        hists,
        context: &ctx,
        refs: Vec::new(),
    };
    for hi in 0..hists.len() {
        if let Some(r) = cur.first(hi, 0) {
            cur.heap.push(r);
        }
    }
    for i in (0..cur.heap.len() / 2).rev() {
        down(&mut cur.heap, i, |a, b| {
            Ok(ctx.compare(
                hists[a.hist as usize]
                    .as_ref()
                    .unwrap()
                    .GetUpper(a.bucket as usize),
                hists[b.hist as usize]
                    .as_ref()
                    .unwrap()
                    .GetUpper(b.bucket as usize),
            )? < 0)
        })?;
    }
    if compact.is_empty() && cur.heap.is_empty() {
        return Ok((
            None,
            NewHistogram(
                first.ID,
                0,
                null,
                first.LastUpdateVersion,
                &first.Tp,
                0,
                size,
            ),
        ));
    }
    let mut top_heap: Vec<Candidate> = Vec::with_capacity(num_top_n as usize);
    let mut candidates = 0;
    let mut ti = 0;
    let mut iteration = 0;
    while ti < compact.len() || !cur.heap.is_empty() {
        if iteration & 1023 == 0 {
            killer.HandleSignal()?;
        }
        iteration += 1;
        let (take_top, take_bucket) = if ti == compact.len() {
            (false, true)
        } else if cur.heap.is_empty() {
            (true, false)
        } else {
            let d = ctx.decode(&compact[ti].encoded, &first.Tp)?;
            let c = ctx.compare(&d, cur.upper(cur.heap[0]))?;
            (c <= 0, c >= 0)
        };
        let mut e = if take_top {
            let e = compact[ti].clone();
            ti += 1;
            e
        } else {
            Candidate {
                encoded: Vec::new(),
                count: 0,
                repeat: 0,
            }
        };
        if take_bucket {
            let (key, repeat) = cur.group(!take_top)?;
            if !take_top {
                e.encoded = key;
            }
            e.repeat = repeat;
            e.count += repeat as u64;
        }
        if e.count > 0 {
            candidates += 1;
            if num_top_n > 0 {
                if top_heap.len() < num_top_n as usize {
                    top_heap.push(e);
                    let j = top_heap.len() - 1;
                    up(&mut top_heap, j, |a, b| Ok(a.count < b.count))?;
                } else if e.count > top_heap[0].count {
                    top_heap[0] = e;
                    down(&mut top_heap, 0, |a, b| Ok(a.count < b.count))?;
                }
            }
        }
    }
    if num_top_n as u64 == vardef::AnalyzeDefaultNumTopN.Load() && candidates > num_top_n as usize {
        top_heap.retain(|e| e.count >= 2);
    }
    let promoted = top_heap
        .iter()
        .map(|e| e.encoded.clone())
        .collect::<HashSet<_>>();
    let removed = top_heap.iter().map(|e| e.repeat).sum::<i64>();
    let global_top = if top_heap.is_empty() {
        None
    } else {
        let mut t = NewTopN(num_top_n as usize);
        for e in top_heap {
            t.AppendTopN(e.encoded, e.count);
        }
        t.Sort();
        Some(t)
    };
    let virtual_entries = compact
        .into_iter()
        .filter(|e| e.count > 0 && !promoted.contains(&e.encoded))
        .collect::<Vec<_>>();
    let total = total - removed + virtual_entries.iter().map(|e| e.count as i64).sum::<i64>();
    let mut refs = MergeRefs {
        refs: cur.refs,
        hists: hists
            .iter()
            .map(|h| h.as_ref().map(Cow::Borrowed))
            .collect(),
        promoted,
        remaining: HashMap::new(),
        effective: HashMap::new(),
        context: &ctx,
    };
    if !virtual_entries.is_empty() {
        refs.merge_virtual(&virtual_entries, first)?;
    }
    let histogram = refs.build(killer, total, null, size, expected_buckets, first)?;
    Ok((global_top, histogram))
}
