use crate::address::PhysAddr;
use crate::error::SvsmError;
use crate::protocols::errors::SvsmReqError;
use crate::protocols::RequestParams;
use crate::mm::set::Set;
use crate::sev::utils::rmp_set_read_only;
use crate::types::{PageSize, PAGE_SIZE, PAGE_SIZE_2M};
use crate::mm::virtualrange::{VIRT_ALIGN_2M, VIRT_ALIGN_4K};
use crate::mm::PerCPUPageMappingGuard;
use crate::mm::guestmem::read_u8;
use crate::mm::{writable_phys_addr, PageBox};
use crate::utils::zero_mem_region;
use crate::locking::SpinLock;

extern crate alloc;
use alloc::vec::Vec;

use core::mem::MaybeUninit;
use core::ptr::NonNull;

const SVSM_FULL_BACKUP: u32 = 0;
const SVSM_RESTORE: u32 = 1;
const SVSM_ENABLE_COPY_ON_WRITE: u32 = 2;
// TODO use after implementing partial backup
//const SVSM_PARTIAL_RESTORE: u32 = 3;

struct MemPage4K<'a> {
    phys_addr: PhysAddr,
    data: &'a mut [u8; PAGE_SIZE],
}

pub static PAGES_TO_BACKUP: Set = Set::new();
pub static PAGES_TO_CLEAR: Set = Set::new();

pub static BACKUP_CREATED: SpinLock<bool> = SpinLock::new(false); 

static BACKUP_PAGES: SpinLock<Vec<MemPage4K<'_>>> = SpinLock::new(Vec::new()); 
static ZERO_PAGES: SpinLock<Vec<PhysAddr>> = SpinLock::new(Vec::new());


pub fn backup_protocol_request(request: u32, _params: &mut RequestParams) -> Result<(), SvsmReqError> {
    match request {
        SVSM_FULL_BACKUP => create_full_backup(),
        SVSM_RESTORE => restore_pages_from_backup(),
        SVSM_ENABLE_COPY_ON_WRITE => enable_copy_on_write(),
        _ => Err(SvsmReqError::unsupported_call()),
    }
}

fn create_full_backup() -> Result<(), SvsmReqError> {
    if *(BACKUP_CREATED.lock()) {
        log::info!("Backup already exists. No new backup will be created.");
        return Ok(());
    }

    log::info!("Starting to backup pages...");
    let mut total_size = 0;
    let mut skipped = 0;
    for (phys_addr, size) in PAGES_TO_BACKUP.iter_addresses() {
        let (size_backed_up, size_skipped) = backup_page(phys_addr, size)?;
        total_size += size_backed_up;
        skipped += size_skipped;
    }
    log::info!("Backed up: {} Byte", total_size);
    log::info!("Skipped: {} Byte", skipped);

    *(BACKUP_CREATED.lock()) = true;
    log::info!("Successfully backed up pages.");
    Ok(())
}

fn backup_page(paddr: PhysAddr, size: PageSize) -> Result<(u64, u64), SvsmError> {
    match size {
        PageSize::Regular => {
            let success = backup_4k_page(paddr)?;
            if success {
                return Ok((PAGE_SIZE as u64, 0))
            } else {
                return Ok((0, PAGE_SIZE as u64))
            }
        }
        PageSize::Huge => {
            let mut start_addr: PhysAddr = paddr;
            let mut backup_size = 0;
            for i in 0..(PAGE_SIZE_2M/PAGE_SIZE) {
                let success = backup_4k_page(start_addr)?;
                if success {
                    backup_size += PAGE_SIZE as u64;
                }
                start_addr = paddr + i * PAGE_SIZE;
            }
            return Ok((backup_size, PAGE_SIZE_2M as u64 - backup_size));
        }
    }
    // TODO verify that data is private (for guest)
}
  
fn backup_4k_page(paddr: PhysAddr) -> Result<bool, SvsmError> {
    let guard = PerCPUPageMappingGuard::create(paddr, paddr+PAGE_SIZE, VIRT_ALIGN_4K)?;
    let virt_addr = guard.virt_addr();
    
    let mut backup = false;
    let page_box_uninit: PageBox<MaybeUninit<[u8; PAGE_SIZE]>> = PageBox::try_new_uninit()?;
    let page_box: PageBox<[u8; PAGE_SIZE]> = unsafe { page_box_uninit.assume_init() };
    let ref_page = PageBox::leak(page_box);
    let mut zero = true;
    for i in 0..(PAGE_SIZE){
        let byte = read_u8(virt_addr+i)?;
        if byte != 0 {
            zero = false;
        }
        ref_page[i] = byte;
    }
    if zero {
        let _ = unsafe {PageBox::from_raw(NonNull::from(ref_page))};
        let mut guard = ZERO_PAGES.lock();
        guard.push(paddr);
    }
    else {
        let mut guard = BACKUP_PAGES.lock();
        guard.push(MemPage4K {
            phys_addr: paddr,
            data: ref_page,
        });
        backup = true;
    }
    Ok(backup)
}

fn restore_pages_from_backup() -> Result<(), SvsmReqError> {
    log::info!("Starting to restore pages from backup");

    log::info!("Restoring non-empty pages...");
    let guard = BACKUP_PAGES.lock();
    for page_src in guard.iter() {
        restore_page(page_src)?;
    }

    log::info!("Restoring empty pages...");
    let guard = ZERO_PAGES.lock();
    for &paddr in guard.iter() {
        zero_page(paddr)?;
    }

    // TODO reset additional pages used by adding them to page to clear
    // TODO flush TLB?
    log::info!("Zeroing new pages...");
    for (_paddr, _size) in PAGES_TO_CLEAR.iter_addresses() {
        // TODO
    }

    log::info!("Successfully restored pages from backup");
    Ok(())
}

fn restore_page(page_src: &MemPage4K<'_>) -> Result<(), SvsmError> {
    let paddr_dest = page_src.phys_addr;
    if !writable_phys_addr(paddr_dest) {
        log::info!("Skipping page {:#x}", paddr_dest);
        return Ok(());
    }
    let guard_cpu = PerCPUPageMappingGuard::create(paddr_dest, paddr_dest+PAGE_SIZE, VIRT_ALIGN_4K)?;
    let virt_addr = guard_cpu.virt_addr();
    unsafe {
        virt_addr.as_mut_ptr::<[u8; PAGE_SIZE]>().write( *page_src.data);
    }
    log::info!("Restored page {:#x}", paddr_dest);
    Ok(())
}

fn zero_page(paddr: PhysAddr) -> Result<(), SvsmError> {
    if !writable_phys_addr(paddr){
        log::info!("Skipping page {:#x}", paddr);
        return Ok(());
    }
    let guard_cpu = PerCPUPageMappingGuard::create(paddr, paddr+PAGE_SIZE, VIRT_ALIGN_4K)?;
    let virt_addr = guard_cpu.virt_addr();
    zero_mem_region(virt_addr, virt_addr+PAGE_SIZE);
    log::info!("Zeroed page {:#x}", paddr);
    Ok(())
}

fn enable_copy_on_write() -> Result<(), SvsmReqError> {
    log::info!("Starting to enable copy-on-write...");
    for (phys_addr, size) in PAGES_TO_BACKUP.iter_addresses() {
        set_read_only(phys_addr, size)?;
    }
    log::info!("Successfully enabled copy-on-write for validated pages");
    Ok(())
}

fn set_read_only(paddr: PhysAddr, size: PageSize) -> Result<(), SvsmError> {
    let guard = match size {
        PageSize::Huge => {
            PerCPUPageMappingGuard::create(paddr, paddr+PAGE_SIZE_2M, VIRT_ALIGN_2M)?
        }
        PageSize::Regular => {
            PerCPUPageMappingGuard::create(paddr, paddr+PAGE_SIZE, VIRT_ALIGN_4K)?
        }
    };
    let virt_addr = guard.virt_addr();
    rmp_set_read_only(virt_addr, size)?;
    log::info!("Set read-only for page {:#x}, size {:?}", paddr, size);
    Ok(())
}
