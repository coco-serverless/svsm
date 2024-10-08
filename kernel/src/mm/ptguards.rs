// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2022-2023 SUSE LLC
//
// Author: Joerg Roedel <jroedel@suse.de>

use super::pagetable::PTEntryFlags;
use crate::address::{Address, PhysAddr, VirtAddr};
use crate::cpu::percpu::this_cpu;
use crate::cpu::tlb::flush_address_sync;
use crate::error::SvsmError;
use crate::mm::virtualrange::{
    virt_alloc_range_2m, virt_alloc_range_4k, virt_free_range_2m, virt_free_range_4k,
};
use crate::types::{PAGE_SIZE, PAGE_SIZE_2M};

use crate::utils::MemoryRegion;

/// Guard for a per-CPU page mapping to ensure adequate cleanup if drop.
#[derive(Debug)]
#[must_use = "if unused the mapping will immediately be unmapped"]
pub struct PerCPUPageMappingGuard {
    mapping: MemoryRegion<VirtAddr>,
    huge: bool,
}

impl PerCPUPageMappingGuard {
    /// Creates a new [`PerCPUPageMappingGuard`] for the specified physical
    /// address range and alignment.
    ///
    /// # Arguments
    ///
    /// * `paddr_start` - The starting physical address of the range.
    /// * `paddr_end` - The ending physical address of the range.
    /// * `alignment` - The desired alignment for the mapping.
    ///
    /// # Returns
    ///
    /// A `Result` containing the [`PerCPUPageMappingGuard`] if successful,
    /// or an `SvsmError` if an error occurs.
    ///
    /// # Panics
    ///
    /// Panics if either `paddr_start`, the size, or `paddr_end`, are not
    /// aligned.
    pub fn create(
        paddr_start: PhysAddr,
        paddr_end: PhysAddr,
        alignment: usize,
    ) -> Result<Self, SvsmError> {
        let align_mask = (PAGE_SIZE << alignment) - 1;
        let size = paddr_end - paddr_start;
        assert!((size & align_mask) == 0);
        assert!((paddr_start.bits() & align_mask) == 0);
        assert!((paddr_end.bits() & align_mask) == 0);

        let flags = PTEntryFlags::data();
        let huge = ((paddr_start.bits() & (PAGE_SIZE_2M - 1)) == 0)
            && ((paddr_end.bits() & (PAGE_SIZE_2M - 1)) == 0);
        let raw_mapping = if huge {
            let region = virt_alloc_range_2m(size, 0)?;
            if let Err(e) = this_cpu()
                .get_pgtable()
                .map_region_2m(region, paddr_start, flags)
            {
                virt_free_range_2m(region);
                return Err(e);
            }
            region
        } else {
            let region = virt_alloc_range_4k(size, 0)?;
            if let Err(e) = this_cpu()
                .get_pgtable()
                .map_region_4k(region, paddr_start, flags)
            {
                virt_free_range_4k(region);
                return Err(e);
            }
            region
        };

        Ok(PerCPUPageMappingGuard {
            mapping: raw_mapping,
            huge,
        })
    }

    /// Creates a new [`PerCPUPageMappingGuard`] for a 4KB page at the
    /// specified physical address, or an `SvsmError` if an error occurs.
    pub fn create_4k(paddr: PhysAddr) -> Result<Self, SvsmError> {
        Self::create(paddr, paddr + PAGE_SIZE, 0)
    }

    /// Returns the virtual address associated with the guard.
    pub fn virt_addr(&self) -> VirtAddr {
        self.mapping.start()
    }
}

impl Drop for PerCPUPageMappingGuard {
    fn drop(&mut self) {
        if self.huge {
            this_cpu().get_pgtable().unmap_region_2m(self.mapping);
            virt_free_range_2m(self.mapping);
        } else {
            this_cpu().get_pgtable().unmap_region_4k(self.mapping);
            virt_free_range_4k(self.mapping);
        }
        flush_address_sync(self.mapping.start());
    }
}
