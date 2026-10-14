//! Whole-system process list from a single NtQuerySystemInformation call.
//! No per-process handles are opened here.
use std::ffi::c_void;
use std::mem::size_of;

const SYSTEM_PROCESS_INFORMATION_CLASS: u32 = 5;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
const INITIAL_BUFFER_BYTES: usize = 512 * 1024;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, return_len: *mut u32) -> i32;
}

#[repr(C)]
struct UnicodeString {
    length_bytes: u16,
    maximum_length_bytes: u16,
    buffer: *const u16,
}

/// x64 layout of SYSTEM_PROCESS_INFORMATION (0x100 bytes); thread entries follow and are skipped.
#[repr(C)]
pub(crate) struct SystemProcessInformation {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawProcess {
    pub pid: u32,
    pub parent_pid: u32,
    pub create_time: i64,
    pub name: String,
    pub session_id: u32,
    pub cpu_time_100ns: u64,
    pub private_working_set: u64,
    pub disk_io_bytes: u64,
    pub threads: u32,
    pub handles: u32,
}

/// Reuses one 8-byte-aligned buffer across reads.
pub struct ProcessReader {
    buf: Vec<u64>,
}

impl Default for ProcessReader {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessReader {
    pub fn new() -> Self {
        Self { buf: vec![0; INITIAL_BUFFER_BYTES / 8] }
    }

    /// Returns every process, or the failing NTSTATUS.
    pub fn read(&mut self) -> Result<Vec<RawProcess>, i32> {
        loop {
            let len_bytes = (self.buf.len() * 8) as u32;
            let mut needed = 0u32;
            // SAFETY: `buf` is a writable, 8-byte-aligned buffer of `len_bytes` bytes.
            let status = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_PROCESS_INFORMATION_CLASS,
                    self.buf.as_mut_ptr().cast(),
                    len_bytes,
                    &mut needed,
                )
            };
            if status == STATUS_INFO_LENGTH_MISMATCH {
                let want_words = (needed as usize + 64 * 1024).div_ceil(8);
                self.buf.resize(want_words.max(self.buf.len() * 2), 0);
                continue;
            }
            if status < 0 {
                return Err(status);
            }
            // SAFETY: the call succeeded, so `buf` holds a valid SystemProcessInformation chain.
            return Ok(unsafe { parse(self.buf.as_ptr().cast(), len_bytes as usize) });
        }
    }
}

/// # Safety
/// `base` must point to `len` bytes written by NtQuerySystemInformation(SystemProcessInformation).
unsafe fn parse(base: *const u8, len: usize) -> Vec<RawProcess> {
    let mut out = Vec::with_capacity(512);
    let mut offset = 0usize;
    while offset + size_of::<SystemProcessInformation>() <= len {
        // SAFETY: bounds checked above; entries are 8-byte aligned by the kernel.
        let info = unsafe { &*(base.add(offset) as *const SystemProcessInformation) };
        let pid = info.unique_process_id as u32;
        let name_units: &[u16] = if info.image_name.buffer.is_null() {
            &[]
        } else {
            // SAFETY: ImageName points inside the same buffer; length is in bytes.
            unsafe {
                std::slice::from_raw_parts(info.image_name.buffer, info.image_name.length_bytes as usize / 2)
            }
        };
        out.push(RawProcess {
            pid,
            parent_pid: info.inherited_from_unique_process_id as u32,
            create_time: info.create_time,
            name: decode_image_name(pid, name_units),
            session_id: info.session_id,
            cpu_time_100ns: (info.user_time.max(0) as u64).saturating_add(info.kernel_time.max(0) as u64),
            private_working_set: info.working_set_private_size.max(0) as u64,
            disk_io_bytes: (info.read_transfer_count.max(0) as u64)
                .saturating_add(info.write_transfer_count.max(0) as u64),
            threads: info.number_of_threads,
            handles: info.handle_count,
        });
        if info.next_entry_offset == 0 {
            break;
        }
        offset += info.next_entry_offset as usize;
    }
    out
}

pub(crate) fn decode_image_name(pid: u32, units: &[u16]) -> String {
    if units.is_empty() {
        return if pid == 0 { "System Idle Process".to_string() } else { format!("<pid {pid}>") };
    }
    String::from_utf16_lossy(units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_layout_matches_x64_ntdll() {
        assert_eq!(size_of::<SystemProcessInformation>(), 0x100);
    }

    #[test]
    fn decode_names() {
        assert_eq!(decode_image_name(0, &[]), "System Idle Process");
        assert_eq!(decode_image_name(1234, &[]), "<pid 1234>");
        let units: Vec<u16> = "a.exe".encode_utf16().collect();
        assert_eq!(decode_image_name(5, &units), "a.exe");
        // Lone surrogate must not panic.
        assert_eq!(decode_image_name(6, &[0x61, 0xD800, 0x62]), "a\u{FFFD}b");
    }
}
