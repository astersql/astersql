// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Condvar, Mutex},
};
/// Coalescing multi-producer/single-consumer notification.
#[derive(Default)]
pub struct Notifier {
    awake: Mutex<bool>,
    wake: Condvar,
}
impl Notifier {
    pub fn wake(&self) {
        let mut a = self.awake.lock().unwrap();
        if !*a {
            *a = true;
            self.wake.notify_one();
        }
    }
    pub fn weak_wake(&self) {
        self.wake()
    }
    pub fn is_awake(&self) -> bool {
        *self.awake.lock().unwrap()
    }
    pub fn clear(&self) -> bool {
        std::mem::replace(&mut *self.awake.lock().unwrap(), false)
    }
    pub fn wait(&self) {
        let a = self.awake.lock().unwrap();
        let mut a = self.wake.wait_while(a, |a| !*a).unwrap();
        *a = false;
    }
}
pub trait Lessable {
    fn less(&self, other: &Self) -> bool;
}
pub struct ItemImpl<V> {
    pub value: V,
    index: Option<usize>,
}
pub type Item<V> = Rc<RefCell<ItemImpl<V>>>;
impl<V> ItemImpl<V> {
    pub fn new(value: V) -> Item<V> {
        Rc::new(RefCell::new(Self { value, index: None }))
    }
}
pub struct PriorityQueue<V: Lessable> {
    items: Vec<Item<V>>,
}
impl<V: Lessable> Default for PriorityQueue<V> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}
impl<V: Lessable> PriorityQueue<V> {
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn front(&self) -> Option<Item<V>> {
        self.items.first().cloned()
    }
    fn less(&self, a: usize, b: usize) -> bool {
        self.items[a]
            .borrow()
            .value
            .less(&self.items[b].borrow().value)
    }
    fn swap(&mut self, a: usize, b: usize) {
        self.items.swap(a, b);
        self.items[a].borrow_mut().index = Some(a);
        self.items[b].borrow_mut().index = Some(b);
    }
    fn up(&mut self, mut i: usize) {
        while i > 0 {
            let p = (i - 1) / 2;
            if !self.less(i, p) {
                break;
            }
            self.swap(i, p);
            i = p;
        }
    }
    fn down(&mut self, mut i: usize) {
        loop {
            let left = i * 2 + 1;
            if left >= self.len() {
                break;
            }
            let mut j = left;
            if left + 1 < self.len() && self.less(left + 1, left) {
                j = left + 1
            }
            if !self.less(j, i) {
                break;
            }
            self.swap(i, j);
            i = j;
        }
    }
    pub fn push(&mut self, value: V) -> Item<V> {
        let item = ItemImpl::new(value);
        let i = self.len();
        item.borrow_mut().index = Some(i);
        self.items.push(item.clone());
        self.up(i);
        item
    }
    fn index(&self, item: Option<&Item<V>>) -> Option<usize> {
        let item = item?;
        let i = item.borrow().index?;
        if i < self.len() && Rc::ptr_eq(item, &self.items[i]) {
            Some(i)
        } else {
            None
        }
    }
    pub fn update(&mut self, item: Option<&Item<V>>, value: V) {
        if let Some(i) = self.index(item) {
            self.items[i].borrow_mut().value = value;
            self.fix(i)
        }
    }
    fn fix(&mut self, i: usize) {
        if i > 0 && self.less(i, (i - 1) / 2) {
            self.up(i)
        } else {
            self.down(i)
        }
    }
    fn remove_index(&mut self, i: usize) -> Item<V> {
        let last = self.len() - 1;
        self.swap(i, last);
        let item = self.items.pop().unwrap();
        item.borrow_mut().index = None;
        if i < self.len() {
            self.fix(i)
        }
        item
    }
    pub fn remove(&mut self, item: Option<&Item<V>>) -> Option<Item<V>> {
        self.index(item).map(|i| self.remove_index(i))
    }
    pub fn pop(&mut self) -> Option<Item<V>> {
        if self.is_empty() {
            None
        } else {
            Some(self.remove_index(0))
        }
    }
}
