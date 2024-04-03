// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) Microsoft Corporation
//
// Author: Jon Lange <jlange@microsoft.com>

use crate::cpu::cpuid::cpuid_table;
use crate::cpu::percpu::PerCpu;
use crate::io::IOPort;
use crate::platform::{PageEncryptionMasks, SvsmPlatform};
use crate::sev::msr_protocol::verify_ghcb_version;
use crate::sev::status::vtom_enabled;
use crate::sev::{sev_status_init, sev_status_verify};
use crate::svsm_console::SVSMIOPort;

static CONSOLE_IO: SVSMIOPort = SVSMIOPort::new();

#[derive(Clone, Copy, Debug)]
pub struct SnpPlatform {}

impl SnpPlatform {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for SnpPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl SvsmPlatform for SnpPlatform {
    fn env_setup(&mut self) {
        sev_status_init();
    }

    fn env_setup_late(&mut self) {
        sev_status_verify();
    }

    fn get_page_encryption_masks(&self, vtom: usize) -> PageEncryptionMasks {
        // Find physical address size.
        let res =
            cpuid_table(0x80000008).expect("Can not get physical address size from CPUID table");
        if vtom_enabled() {
            PageEncryptionMasks {
                private_pte_mask: 0,
                shared_pte_mask: vtom,
                addr_mask_width: vtom.leading_zeros(),
                phys_addr_sizes: res.eax,
            }
        } else {
            // Find C-bit position.
            let res = cpuid_table(0x8000001f).expect("Can not get C-Bit position from CPUID table");
            let c_bit = res.ebx & 0x3f;
            PageEncryptionMasks {
                private_pte_mask: 1 << c_bit,
                shared_pte_mask: 0,
                addr_mask_width: c_bit,
                phys_addr_sizes: res.eax,
            }
        }
    }

    fn setup_guest_host_comm(&mut self, cpu: &mut PerCpu, is_bsp: bool) {
        if is_bsp {
            verify_ghcb_version();
        }

        cpu.setup_ghcb().unwrap_or_else(|_| {
            if is_bsp {
                panic!("Failed to setup BSP GHCB");
            } else {
                panic!("Failed to setup AP GHCB");
            }
        });
        cpu.register_ghcb().expect("Failed to register GHCB");
    }

    fn get_console_io_port(&self) -> &'static dyn IOPort {
        &CONSOLE_IO
    }
}
