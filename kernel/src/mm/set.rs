extern crate alloc;
use alloc::collections::BTreeSet;
use crate::locking::SpinLock;
use crate::address::PhysAddr;
use crate::types::PageSize;

#[derive(Debug)]
pub struct Set {
    set: SpinLock<BTreeSet<(PhysAddr, PageSize)>>
}

impl Set {
    pub const fn new() -> Self {
        Self {
            set: SpinLock::new(BTreeSet::new())
        }
    }
    
    pub fn insert_addr(&self, value: PhysAddr, size: PageSize) {
        log::info!("Inserting address {:#x} with size {:?}", value, size);
        let mut guard = self.set.lock();
        guard.insert((value, size));
    }

    pub fn remove_addr(&self, value: PhysAddr, size: PageSize) -> bool {
        let mut guard = self.set.lock();
        guard.remove(&(value, size))
    }

    pub fn contains_addr(&self, value: PhysAddr, size: PageSize) -> bool {
        let guard = self.set.lock();
        guard.contains(&(value, size))
    }

    pub fn iter_addresses(&self) -> impl Iterator<Item = (PhysAddr, PageSize)> {
        let guard = self.set.lock();
        let cloned_set = guard.clone();
        cloned_set.into_iter()
    }

    pub fn size(&self) -> usize {
        let guard = self.set.lock();
        guard.len()
    }
}
