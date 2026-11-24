use std::fmt;
use std::ptr::NonNull;

use toluca_core::GuestAddr;
use toluca_core::memory::{
    GUEST_ARENA_SIZE, GUEST_PAGE_SIZE, GUEST_PEB_ADDR, GUEST_STACK_BASE, GUEST_STACK_SIZE,
    GUEST_TEB_ADDR, NULL_GUARD_END,
};
use toluca_core::pe::PeImage;

const GUEST_PAGE_SHIFT: u32 = 12;
const BITS_PER_WORD: usize = 64;
const TOTAL_GUEST_PAGES: usize = (GUEST_ARENA_SIZE / GUEST_PAGE_SIZE as u64) as usize;
const BITSET_WORDS: usize = TOTAL_GUEST_PAGES / BITS_PER_WORD;
const INITIAL_STACK_COMMIT_BYTES: u32 = 0x0010_0000;

const TEB_SEH_SENTINEL: u32 = 0xFFFF_FFFF;
const TEB_OFF_SEH_FRAME: u32 = 0x00;
const TEB_OFF_STACK_BASE: u32 = 0x04;
const TEB_OFF_STACK_LIMIT: u32 = 0x08;
const TEB_OFF_SELF: u32 = 0x18;
const TEB_OFF_PEB: u32 = 0x30;
const PEB_OFF_IMAGE_BASE: u32 = 0x08;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArenaError {
    UnsupportedHostPageSize(i64),
    ArenaSizeUnsupported,
    MmapFailed(i32),
    MprotectFailed(i32),
    MadviseFailed(i32),
    NullGuardViolation(GuestAddr),
    AddressOverflow { addr: GuestAddr, len: usize },
    Unmapped(GuestAddr),
    ReadOnly(GuestAddr),
    UnterminatedCString { addr: GuestAddr, max_len: usize },
    StackOverflow(GuestAddr),
}

impl fmt::Display for ArenaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedHostPageSize(sz) => {
                write!(f, "unsupported host page size {sz} bytes")
            }
            Self::ArenaSizeUnsupported => {
                write!(f, "host pointer width cannot map a 4 GiB guest arena")
            }
            Self::MmapFailed(errno) => write!(f, "mmap(4 GiB) failed with errno {errno}"),
            Self::MprotectFailed(errno) => write!(f, "mprotect failed with errno {errno}"),
            Self::MadviseFailed(errno) => write!(f, "madvise(MADV_DONTNEED) failed with errno {errno}"),
            Self::NullGuardViolation(addr) => {
                write!(f, "guest address 0x{addr:08x} falls inside the 64 KiB null guard")
            }
            Self::AddressOverflow { addr, len } => {
                write!(f, "guest range 0x{addr:08x} + {len:#x} overflows 32-bit address space")
            }
            Self::Unmapped(addr) => write!(f, "unmapped guest memory access at 0x{addr:08x}"),
            Self::ReadOnly(addr) => write!(f, "write to read-only guest memory at 0x{addr:08x}"),
            Self::UnterminatedCString { addr, max_len } => {
                write!(
                    f,
                    "unterminated guest C string at 0x{addr:08x} within {max_len} bytes"
                )
            }
            Self::StackOverflow(addr) => {
                write!(f, "guest stack commit address 0x{addr:08x} is outside stack bounds")
            }
        }
    }
}

impl std::error::Error for ArenaError {}

struct PageBitset {
    words: Box<[u64]>,
}

impl PageBitset {
    fn new() -> Self {
        Self {
            words: vec![0u64; BITSET_WORDS].into_boxed_slice(),
        }
    }

    fn get(&self, page_idx: usize) -> bool {
        let word = page_idx / BITS_PER_WORD;
        let bit = page_idx % BITS_PER_WORD;
        (self.words[word] & (1u64 << bit)) != 0
    }

    fn set_range(&mut self, start_page: usize, end_page: usize, value: bool) {
        for_each_word_mask(start_page, end_page, |word_idx, mask| {
            if value {
                self.words[word_idx] |= mask;
            } else {
                self.words[word_idx] &= !mask;
            }
        });
    }

    fn any_in_range(&self, start_page: usize, end_page: usize) -> bool {
        let mut found = false;
        for_each_word_mask(start_page, end_page, |word_idx, mask| {
            if (self.words[word_idx] & mask) != 0 {
                found = true;
            }
        });
        found
    }

    fn all_in_range(&self, start_page: usize, end_page: usize) -> bool {
        let mut all_set = true;
        for_each_word_mask(start_page, end_page, |word_idx, mask| {
            if (self.words[word_idx] & mask) != mask {
                all_set = false;
            }
        });
        all_set
    }
}

fn word_mask(start_bit: usize, end_bit: usize) -> u64 {
    let count = end_bit - start_bit;
    if count >= BITS_PER_WORD {
        u64::MAX
    } else {
        ((1u64 << count) - 1) << start_bit
    }
}

fn for_each_word_mask<F>(start_page: usize, end_page: usize, mut f: F)
where
    F: FnMut(usize, u64),
{
    if start_page >= end_page {
        return;
    }
    let first_word = start_page / BITS_PER_WORD;
    let last_word = (end_page - 1) / BITS_PER_WORD;
    for word_idx in first_word..=last_word {
        let lo = if word_idx == first_word {
            start_page % BITS_PER_WORD
        } else {
            0
        };
        let hi = if word_idx == last_word {
            ((end_page - 1) % BITS_PER_WORD) + 1
        } else {
            BITS_PER_WORD
        };
        f(word_idx, word_mask(lo, hi));
    }
}

pub struct GuestArena {
    base: NonNull<u8>,
    size: usize,
    host_page_size: usize,
    committed: PageBitset,
    writable: PageBitset,
}

unsafe impl Send for GuestArena {}

impl GuestArena {
    pub fn new() -> Result<Self, ArenaError> {
        let size = usize::try_from(GUEST_ARENA_SIZE).map_err(|_| ArenaError::ArenaSizeUnsupported)?;
        let host_page_size = query_host_page_size()?;

        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(ArenaError::MmapFailed(last_errno()));
        }
        let base = NonNull::new(ptr.cast::<u8>()).ok_or(ArenaError::MmapFailed(0))?;

        Ok(Self {
            base,
            size,
            host_page_size,
            committed: PageBitset::new(),
            writable: PageBitset::new(),
        })
    }

    pub fn base_ptr(&self) -> *mut u8 {
        self.base.as_ptr()
    }

    pub fn host_page_size(&self) -> usize {
        self.host_page_size
    }

    pub fn commit_range(
        &mut self,
        addr: GuestAddr,
        len: usize,
        writable: bool,
    ) -> Result<(), ArenaError> {
        let end = validate_mutation_range(addr, len)?;
        if len == 0 {
            return Ok(());
        }

        let (guest_start, guest_end) = guest_page_bounds(addr as u64, end);
        let start_page = (guest_start >> GUEST_PAGE_SHIFT) as usize;
        let end_page = (guest_end >> GUEST_PAGE_SHIFT) as usize;

        let host_size = self.host_page_size as u64;
        let host_start = align_down_u64(guest_start, host_size);
        let host_end = align_up_u64(guest_end, host_size);

        let mut hp = host_start;
        while hp < host_end {
            let run_writable = self.host_page_would_be_writable(hp, start_page, end_page, writable);
            let mut run_end = hp + host_size;
            while run_end < host_end
                && self.host_page_would_be_writable(run_end, start_page, end_page, writable)
                    == run_writable
            {
                run_end += host_size;
            }

            let prot = if run_writable {
                libc::PROT_READ | libc::PROT_WRITE
            } else {
                libc::PROT_READ
            };
            self.mprotect_range(hp, (run_end - hp) as usize, prot)?;
            hp = run_end;
        }

        self.committed.set_range(start_page, end_page, true);
        self.writable.set_range(start_page, end_page, writable);
        Ok(())
    }

    pub fn decommit_range(&mut self, addr: GuestAddr, len: usize) -> Result<(), ArenaError> {
        let end = validate_mutation_range(addr, len)?;
        if len == 0 {
            return Ok(());
        }

        let (guest_start, guest_end) = guest_page_bounds(addr as u64, end);
        let start_page = (guest_start >> GUEST_PAGE_SHIFT) as usize;
        let end_page = (guest_end >> GUEST_PAGE_SHIFT) as usize;

        self.committed.set_range(start_page, end_page, false);
        self.writable.set_range(start_page, end_page, false);

        let host_size = self.host_page_size as u64;
        let host_start = align_down_u64(guest_start, host_size);
        let host_end = align_up_u64(guest_end, host_size);
        let pages_per_host = self.host_page_size >> GUEST_PAGE_SHIFT;

        let mut hp = host_start;
        while hp < host_end {
            let hp_start_page = (hp >> GUEST_PAGE_SHIFT) as usize;
            let hp_end_page = hp_start_page + pages_per_host;

            if !self.committed.any_in_range(hp_start_page, hp_end_page) {
                let mut run_end = hp + host_size;
                while run_end < host_end {
                    let next_start = (run_end >> GUEST_PAGE_SHIFT) as usize;
                    if self.committed.any_in_range(next_start, next_start + pages_per_host) {
                        break;
                    }
                    run_end += host_size;
                }
                let run_len = (run_end - hp) as usize;
                self.madvise_dontneed(hp, run_len)?;
                self.mprotect_range(hp, run_len, libc::PROT_NONE)?;
                hp = run_end;
            } else {
                if !self.writable.any_in_range(hp_start_page, hp_end_page) {
                    self.mprotect_range(hp, self.host_page_size, libc::PROT_READ)?;
                }
                hp += host_size;
            }
        }

        Ok(())
    }

    pub fn is_range_committed(&self, addr: GuestAddr, len: usize) -> bool {
        if addr < NULL_GUARD_END {
            return false;
        }
        if len == 0 {
            return self.committed.get((addr as usize) >> GUEST_PAGE_SHIFT);
        }
        let Some(end) = (addr as u64)
            .checked_add(len as u64)
            .filter(|&e| e <= GUEST_ARENA_SIZE)
        else {
            return false;
        };
        let start_page = (addr as usize) >> GUEST_PAGE_SHIFT;
        let end_page = ((end - 1) as usize >> GUEST_PAGE_SHIFT) + 1;
        self.committed.all_in_range(start_page, end_page)
    }

    pub fn read_bytes(&self, addr: GuestAddr, len: usize) -> Result<&[u8], ArenaError> {
        self.ensure_readable(addr, len)?;
        if len == 0 {
            return Ok(&[]);
        }
        unsafe {
            let ptr = self.base.as_ptr().add(addr as usize);
            Ok(std::slice::from_raw_parts(ptr, len))
        }
    }

    pub fn write_bytes(&mut self, addr: GuestAddr, data: &[u8]) -> Result<(), ArenaError> {
        self.ensure_writable(addr, data.len())?;
        if data.is_empty() {
            return Ok(());
        }
        unsafe {
            let dst = self.base.as_ptr().add(addr as usize);
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        Ok(())
    }

    pub fn write_slice_mut(&mut self, addr: GuestAddr, len: usize) -> Result<&mut [u8], ArenaError> {
        self.ensure_writable(addr, len)?;
        if len == 0 {
            return Ok(&mut []);
        }
        unsafe {
            let dst = self.base.as_ptr().add(addr as usize);
            Ok(std::slice::from_raw_parts_mut(dst, len))
        }
    }

    pub fn zero_range(&mut self, addr: GuestAddr, len: usize) -> Result<(), ArenaError> {
        self.ensure_writable(addr, len)?;
        if len == 0 {
            return Ok(());
        }
        unsafe {
            let dst = self.base.as_ptr().add(addr as usize);
            std::ptr::write_bytes(dst, 0, len);
        }
        Ok(())
    }

    pub fn copy_bytes(
        &mut self,
        dst: GuestAddr,
        src: GuestAddr,
        len: usize,
    ) -> Result<(), ArenaError> {
        self.ensure_readable(src, len)?;
        self.ensure_writable(dst, len)?;
        if len == 0 || dst == src {
            return Ok(());
        }
        unsafe {
            let src_ptr = self.base.as_ptr().add(src as usize);
            let dst_ptr = self.base.as_ptr().add(dst as usize);
            std::ptr::copy(src_ptr, dst_ptr, len);
        }
        Ok(())
    }

    pub fn read_u8(&self, addr: GuestAddr) -> Result<u8, ArenaError> {
        Ok(self.read_bytes(addr, 1)?[0])
    }

    pub fn read_u16(&self, addr: GuestAddr) -> Result<u16, ArenaError> {
        let bytes = self.read_bytes(addr, 2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn read_u32(&self, addr: GuestAddr) -> Result<u32, ArenaError> {
        let bytes = self.read_bytes(addr, 4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn read_u64(&self, addr: GuestAddr) -> Result<u64, ArenaError> {
        let bytes = self.read_bytes(addr, 8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub fn read_i32(&self, addr: GuestAddr) -> Result<i32, ArenaError> {
        Ok(self.read_u32(addr)? as i32)
    }

    pub fn write_u8(&mut self, addr: GuestAddr, val: u8) -> Result<(), ArenaError> {
        self.write_bytes(addr, &[val])
    }

    pub fn write_u16(&mut self, addr: GuestAddr, val: u16) -> Result<(), ArenaError> {
        self.write_bytes(addr, &val.to_le_bytes())
    }

    pub fn write_u32(&mut self, addr: GuestAddr, val: u32) -> Result<(), ArenaError> {
        self.write_bytes(addr, &val.to_le_bytes())
    }

    pub fn write_u64(&mut self, addr: GuestAddr, val: u64) -> Result<(), ArenaError> {
        self.write_bytes(addr, &val.to_le_bytes())
    }

    pub fn write_iat_u32(&mut self, addr: GuestAddr, val: u32) -> Result<(), ArenaError> {
        self.ensure_readable(addr, 4)?;
        let start_page = (addr as usize) >> GUEST_PAGE_SHIFT;
        let end_page = (((addr as u64 + 3) as usize) >> GUEST_PAGE_SHIFT) + 1;
        if self.writable.all_in_range(start_page, end_page) {
            return self.write_u32(addr, val);
        }

        let was_writable: [bool; 2] = [
            self.writable.get(start_page),
            start_page + 1 < end_page && self.writable.get(start_page + 1),
        ];

        self.commit_range(addr, 4, true)?;
        let write_result = self.write_u32(addr, val);

        for (offset, page) in (start_page..end_page).enumerate() {
            if !was_writable[offset] {
                let page_addr = (page as GuestAddr) << GUEST_PAGE_SHIFT;
                let _ = self.commit_range(page_addr, GUEST_PAGE_SIZE as usize, false);
            }
        }
        write_result
    }

    pub fn read_cstr(&self, addr: GuestAddr, max_len: usize) -> Result<String, ArenaError> {
        if addr < NULL_GUARD_END {
            return Err(ArenaError::Unmapped(addr));
        }

        let mut bytes = Vec::new();
        let mut offset = 0usize;

        while offset < max_len {
            let cur = addr
                .checked_add(offset as u32)
                .ok_or(ArenaError::Unmapped(addr))?;
            if cur < NULL_GUARD_END {
                return Err(ArenaError::Unmapped(cur));
            }
            let page_idx = (cur as usize) >> GUEST_PAGE_SHIFT;
            if !self.committed.get(page_idx) {
                return Err(ArenaError::Unmapped(cur));
            }

            let page_rem = (GUEST_PAGE_SIZE as usize) - ((cur as usize) & (GUEST_PAGE_SIZE as usize - 1));
            let chunk_len = page_rem.min(max_len - offset);
            let slice = unsafe {
                let ptr = self.base.as_ptr().add(cur as usize);
                std::slice::from_raw_parts(ptr, chunk_len)
            };

            if let Some(nul_pos) = slice.iter().position(|&b| b == 0) {
                bytes.extend_from_slice(&slice[..nul_pos]);
                return Ok(String::from_utf8_lossy(&bytes).into_owned());
            }

            bytes.extend_from_slice(slice);
            offset += chunk_len;
        }

        Err(ArenaError::UnterminatedCString { addr, max_len })
    }

    pub fn write_cstr(&mut self, addr: GuestAddr, s: &str) -> Result<(), ArenaError> {
        let total_len = s
            .len()
            .checked_add(1)
            .ok_or(ArenaError::AddressOverflow { addr, len: s.len() })?;
        self.ensure_writable(addr, total_len)?;
        unsafe {
            let dst = self.base.as_ptr().add(addr as usize);
            std::ptr::copy_nonoverlapping(s.as_ptr(), dst, s.len());
            *dst.add(s.len()) = 0;
        }
        Ok(())
    }

    pub fn map_pe_image(&mut self, pe: &PeImage, raw_pe: &[u8]) -> Result<(), ArenaError> {
        if pe.image_base < NULL_GUARD_END {
            return Err(ArenaError::NullGuardViolation(pe.image_base));
        }

        let header_len = pe.size_of_headers as usize;
        if header_len > 0 {
            self.commit_range(pe.image_base, header_len, true)?;
            self.zero_range(pe.image_base, header_len)?;
            let copy_len = header_len.min(raw_pe.len());
            self.write_bytes(pe.image_base, &raw_pe[..copy_len])?;
        }

        for section in &pe.sections {
            let mapped_size = section.mapped_size() as usize;
            if mapped_size == 0 {
                continue;
            }
            let sec_va = pe
                .image_base
                .checked_add(section.virtual_address)
                .ok_or(ArenaError::AddressOverflow {
                    addr: pe.image_base,
                    len: section.virtual_address as usize,
                })?;

            self.commit_range(sec_va, mapped_size, true)?;
            self.zero_range(sec_va, mapped_size)?;

            let raw = section.raw_bytes(raw_pe);
            let copy_len = raw.len().min(mapped_size);
            self.write_bytes(sec_va, &raw[..copy_len])?;
        }

        if header_len > 0 && !range_overlaps_writable_section(pe, pe.image_base, header_len) {
            self.commit_range(pe.image_base, header_len, false)?;
        }

        for section in &pe.sections {
            let mapped_size = section.mapped_size() as usize;
            if mapped_size == 0 || section.is_writable() {
                continue;
            }
            let sec_va = pe.image_base + section.virtual_address;
            if !range_overlaps_other_writable_section(pe, sec_va, mapped_size, section) {
                self.commit_range(sec_va, mapped_size, false)?;
            }
        }

        Ok(())
    }

    pub fn init_thread_environment(&mut self, image_base: GuestAddr) -> Result<(), ArenaError> {
        let stack_top = GUEST_STACK_BASE + GUEST_STACK_SIZE;

        self.commit_range(GUEST_TEB_ADDR, GUEST_PAGE_SIZE as usize, true)?;
        self.zero_range(GUEST_TEB_ADDR, GUEST_PAGE_SIZE as usize)?;
        self.write_u32(GUEST_TEB_ADDR + TEB_OFF_SEH_FRAME, TEB_SEH_SENTINEL)?;
        self.write_u32(GUEST_TEB_ADDR + TEB_OFF_STACK_BASE, stack_top)?;
        self.write_u32(GUEST_TEB_ADDR + TEB_OFF_STACK_LIMIT, GUEST_STACK_BASE)?;
        self.write_u32(GUEST_TEB_ADDR + TEB_OFF_SELF, GUEST_TEB_ADDR)?;
        self.write_u32(GUEST_TEB_ADDR + TEB_OFF_PEB, GUEST_PEB_ADDR)?;

        self.commit_range(GUEST_PEB_ADDR, GUEST_PAGE_SIZE as usize, true)?;
        self.zero_range(GUEST_PEB_ADDR, GUEST_PAGE_SIZE as usize)?;
        self.write_u32(GUEST_PEB_ADDR + PEB_OFF_IMAGE_BASE, image_base)?;

        let initial_bottom = stack_top - INITIAL_STACK_COMMIT_BYTES;
        self.commit_range(initial_bottom, INITIAL_STACK_COMMIT_BYTES as usize, true)?;
        Ok(())
    }

    pub fn commit_stack_down_to(&mut self, addr: GuestAddr) -> Result<(), ArenaError> {
        let stack_top = GUEST_STACK_BASE + GUEST_STACK_SIZE;
        if addr < GUEST_STACK_BASE || addr >= stack_top {
            return Err(ArenaError::StackOverflow(addr));
        }
        let aligned_addr = align_down_u64(addr as u64, GUEST_PAGE_SIZE as u64) as GuestAddr;
        let len = (stack_top - aligned_addr) as usize;
        if self.is_range_committed(aligned_addr, len) {
            return Ok(());
        }
        self.commit_range(aligned_addr, len, true)
    }

    fn host_page_would_be_writable(
        &self,
        host_page_addr: u64,
        new_start_page: usize,
        new_end_page: usize,
        new_writable: bool,
    ) -> bool {
        let hp_start_page = (host_page_addr >> GUEST_PAGE_SHIFT) as usize;
        let hp_end_page = hp_start_page + (self.host_page_size >> GUEST_PAGE_SHIFT);

        if new_writable && new_start_page < hp_end_page && new_end_page > hp_start_page {
            return true;
        }
        if hp_start_page < new_start_page && self.writable.any_in_range(hp_start_page, new_start_page) {
            return true;
        }
        if hp_end_page > new_end_page && self.writable.any_in_range(new_end_page, hp_end_page) {
            return true;
        }
        false
    }

    fn ensure_readable(&self, addr: GuestAddr, len: usize) -> Result<(), ArenaError> {
        if addr < NULL_GUARD_END {
            return Err(ArenaError::Unmapped(addr));
        }
        if len == 0 {
            return Ok(());
        }
        let end = (addr as u64)
            .checked_add(len as u64)
            .filter(|&e| e <= GUEST_ARENA_SIZE)
            .ok_or(ArenaError::Unmapped(addr))?;
        let start_page = (addr as usize) >> GUEST_PAGE_SHIFT;
        let end_page = ((end - 1) as usize >> GUEST_PAGE_SHIFT) + 1;
        if !self.committed.all_in_range(start_page, end_page) {
            return Err(ArenaError::Unmapped(self.first_unmapped(addr, start_page, end_page)));
        }
        Ok(())
    }

    fn ensure_writable(&self, addr: GuestAddr, len: usize) -> Result<(), ArenaError> {
        self.ensure_readable(addr, len)?;
        if len == 0 {
            return Ok(());
        }
        let end = addr as u64 + len as u64;
        let start_page = (addr as usize) >> GUEST_PAGE_SHIFT;
        let end_page = ((end - 1) as usize >> GUEST_PAGE_SHIFT) + 1;
        if !self.writable.all_in_range(start_page, end_page) {
            return Err(ArenaError::ReadOnly(addr));
        }
        Ok(())
    }

    fn first_unmapped(&self, addr: GuestAddr, start_page: usize, end_page: usize) -> GuestAddr {
        for page in start_page..end_page {
            if !self.committed.get(page) {
                let page_addr = (page as GuestAddr) << GUEST_PAGE_SHIFT;
                return page_addr.max(addr);
            }
        }
        addr
    }

    fn mprotect_range(&self, guest_offset: u64, len: usize, prot: libc::c_int) -> Result<(), ArenaError> {
        let ptr = unsafe { self.base.as_ptr().add(guest_offset as usize) };
        let rc = unsafe { libc::mprotect(ptr.cast(), len, prot) };
        if rc != 0 {
            return Err(ArenaError::MprotectFailed(last_errno()));
        }
        Ok(())
    }

    fn madvise_dontneed(&self, guest_offset: u64, len: usize) -> Result<(), ArenaError> {
        let ptr = unsafe { self.base.as_ptr().add(guest_offset as usize) };
        let rc = unsafe { libc::madvise(ptr.cast(), len, libc::MADV_DONTNEED) };
        if rc != 0 {
            return Err(ArenaError::MadviseFailed(last_errno()));
        }
        Ok(())
    }
}

impl Drop for GuestArena {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.base.as_ptr().cast(), self.size);
        }
    }
}

fn query_host_page_size() -> Result<usize, ArenaError> {
    let raw = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if raw <= 0 {
        return Err(ArenaError::UnsupportedHostPageSize(raw));
    }
    let size = usize::try_from(raw).map_err(|_| ArenaError::UnsupportedHostPageSize(raw))?;
    if !size.is_power_of_two() || size < GUEST_PAGE_SIZE as usize {
        return Err(ArenaError::UnsupportedHostPageSize(raw));
    }
    Ok(size)
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn validate_mutation_range(addr: GuestAddr, len: usize) -> Result<u64, ArenaError> {
    if addr < NULL_GUARD_END {
        return Err(ArenaError::NullGuardViolation(addr));
    }
    let end = (addr as u64)
        .checked_add(len as u64)
        .filter(|&e| e <= GUEST_ARENA_SIZE)
        .ok_or(ArenaError::AddressOverflow { addr, len })?;
    Ok(end)
}

fn guest_page_bounds(start: u64, end: u64) -> (u64, u64) {
    let page = GUEST_PAGE_SIZE as u64;
    (align_down_u64(start, page), align_up_u64(end, page))
}

const fn align_down_u64(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

const fn align_up_u64(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

fn page_span(addr: GuestAddr, len: usize) -> (u64, u64) {
    let start = align_down_u64(addr as u64, GUEST_PAGE_SIZE as u64);
    let end = align_up_u64(addr as u64 + len as u64, GUEST_PAGE_SIZE as u64);
    (start, end)
}

fn range_overlaps_writable_section(pe: &PeImage, addr: GuestAddr, len: usize) -> bool {
    let (start_a, end_a) = page_span(addr, len);
    pe.sections.iter().any(|sec| {
        if !sec.is_writable() || sec.mapped_size() == 0 {
            return false;
        }
        let (start_b, end_b) = page_span(pe.image_base + sec.virtual_address, sec.mapped_size() as usize);
        start_a < end_b && end_a > start_b
    })
}

fn range_overlaps_other_writable_section(
    pe: &PeImage,
    addr: GuestAddr,
    len: usize,
    current: &toluca_core::pe::PeSection,
) -> bool {
    let (start_a, end_a) = page_span(addr, len);
    pe.sections.iter().any(|sec| {
        if std::ptr::eq(sec, current) || !sec.is_writable() || sec.mapped_size() == 0 {
            return false;
        }
        let (start_b, end_b) = page_span(pe.image_base + sec.virtual_address, sec.mapped_size() as usize);
        start_a < end_b && end_a > start_b
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use toluca_core::memory::DEFAULT_IMAGE_BASE;
    use toluca_core::pe::test_helpers::{TestSectionSpec, build_test_pe32};
    use toluca_core::pe::{
        IMAGE_SCN_CNT_CODE, IMAGE_SCN_MEM_EXECUTE, IMAGE_SCN_MEM_READ, IMAGE_SCN_MEM_WRITE,
    };

    #[test]
    fn rejects_null_guard_and_overflow_ranges() {
        let mut arena = GuestArena::new().unwrap();
        assert_eq!(
            arena.commit_range(0x0000_1000, 0x1000, true).unwrap_err(),
            ArenaError::NullGuardViolation(0x0000_1000)
        );
        assert_eq!(
            arena.commit_range(0xFFFF_F000, 0x2000, true).unwrap_err(),
            ArenaError::AddressOverflow {
                addr: 0xFFFF_F000,
                len: 0x2000
            }
        );
        assert_eq!(
            arena.read_u32(0x0000_0010).unwrap_err(),
            ArenaError::Unmapped(0x0000_0010)
        );
        assert_eq!(
            arena.read_u32(0x2000_0000).unwrap_err(),
            ArenaError::Unmapped(0x2000_0000)
        );
    }

    #[test]
    fn commit_read_write_and_decommit() {
        let mut arena = GuestArena::new().unwrap();
        let base = 0x1000_0000;
        let span = arena.host_page_size().max(0x4000);
        arena.commit_range(base, span, true).unwrap();
        assert!(arena.is_range_committed(base, span));

        arena.write_u8(base, 0xAB).unwrap();
        arena.write_u16(base + 3, 0xBEEF).unwrap();
        arena.write_u32(base + 8, 0xDEAD_C0DE).unwrap();
        arena.write_u64(base + 16, 0x0102_0304_0506_0708).unwrap();
        arena.write_cstr(base + 32, "TolucaEngine").unwrap();

        assert_eq!(arena.read_u8(base).unwrap(), 0xAB);
        assert_eq!(arena.read_u16(base + 3).unwrap(), 0xBEEF);
        assert_eq!(arena.read_u32(base + 8).unwrap(), 0xDEAD_C0DE);
        assert_eq!(arena.read_i32(base + 8).unwrap(), 0xDEAD_C0DEu32 as i32);
        assert_eq!(arena.read_u64(base + 16).unwrap(), 0x0102_0304_0506_0708);
        assert_eq!(arena.read_cstr(base + 32, 64).unwrap(), "TolucaEngine");
        assert_eq!(
            arena.read_cstr(base + 32, 4).unwrap_err(),
            ArenaError::UnterminatedCString {
                addr: base + 32,
                max_len: 4
            }
        );

        arena.decommit_range(base, span).unwrap();
        assert!(!arena.is_range_committed(base, span));
        assert_eq!(arena.read_u32(base).unwrap_err(), ArenaError::Unmapped(base));
    }

    #[test]
    fn maps_synthetic_pe_with_text_and_bss() {
        let mut arena = GuestArena::new().unwrap();
        let code = [0x55u8, 0x89, 0xE5, 0xC3];
        let mut raw_pe = build_test_pe32(
            DEFAULT_IMAGE_BASE,
            0x1000,
            0x0102,
            &[],
            &[
                TestSectionSpec {
                    name: ".text",
                    virtual_address: 0x1000,
                    characteristics: IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ,
                    data: &code,
                },
                TestSectionSpec {
                    name: ".bss",
                    virtual_address: 0x2000,
                    characteristics: IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
                    data: &[],
                },
            ],
        );

        // Patch .bss section header in the synthetic PE: VirtualSize = 0x2000, SizeOfRawData = 0
        let opt_size = 96 + 16 * 8;
        let sec_table = 0x80 + 4 + 20 + opt_size;
        let bss_header = sec_table + 40;
        raw_pe[bss_header + 8..bss_header + 12].copy_from_slice(&0x2000u32.to_le_bytes());
        raw_pe[bss_header + 16..bss_header + 20].copy_from_slice(&0u32.to_le_bytes());

        let pe = PeImage::parse(raw_pe.clone()).unwrap();
        arena.map_pe_image(&pe, &raw_pe).unwrap();

        // Headers are mapped at image_base and start with MZ
        assert_eq!(arena.read_u16(DEFAULT_IMAGE_BASE).unwrap(), 0x5A4D);
        // .text bytes are mapped and read-only
        assert_eq!(arena.read_bytes(DEFAULT_IMAGE_BASE + 0x1000, 4).unwrap(), &code);
        assert_eq!(
            arena.write_u8(DEFAULT_IMAGE_BASE + 0x1000, 0x90).unwrap_err(),
            ArenaError::ReadOnly(DEFAULT_IMAGE_BASE + 0x1000)
        );
        // .bss is zero-filled and writable across the full 0x2000 bytes
        let bss_start = DEFAULT_IMAGE_BASE + 0x2000;
        assert_eq!(arena.read_u32(bss_start + 0x1FFC).unwrap(), 0);
        arena.write_u32(bss_start + 0x1FFC, 0x1234_5678).unwrap();
        assert_eq!(arena.read_u32(bss_start + 0x1FFC).unwrap(), 0x1234_5678);
    }

    #[test]
    fn initializes_teb_peb_and_stack() {
        let mut arena = GuestArena::new().unwrap();
        arena.init_thread_environment(DEFAULT_IMAGE_BASE).unwrap();

        let stack_top = GUEST_STACK_BASE + GUEST_STACK_SIZE;
        assert_eq!(arena.read_u32(GUEST_TEB_ADDR + TEB_OFF_SEH_FRAME).unwrap(), TEB_SEH_SENTINEL);
        assert_eq!(arena.read_u32(GUEST_TEB_ADDR + TEB_OFF_STACK_BASE).unwrap(), stack_top);
        assert_eq!(arena.read_u32(GUEST_TEB_ADDR + TEB_OFF_STACK_LIMIT).unwrap(), GUEST_STACK_BASE);
        assert_eq!(arena.read_u32(GUEST_TEB_ADDR + TEB_OFF_SELF).unwrap(), GUEST_TEB_ADDR);
        assert_eq!(arena.read_u32(GUEST_TEB_ADDR + TEB_OFF_PEB).unwrap(), GUEST_PEB_ADDR);
        assert_eq!(arena.read_u32(GUEST_PEB_ADDR + PEB_OFF_IMAGE_BASE).unwrap(), DEFAULT_IMAGE_BASE);

        assert!(arena.is_range_committed(stack_top - INITIAL_STACK_COMMIT_BYTES, INITIAL_STACK_COMMIT_BYTES as usize));
        assert!(!arena.is_range_committed(GUEST_STACK_BASE, GUEST_PAGE_SIZE as usize));

        arena.commit_stack_down_to(GUEST_STACK_BASE + 0x1000).unwrap();
        assert!(arena.is_range_committed(GUEST_STACK_BASE + 0x1000, 0x1000));
        assert_eq!(
            arena.commit_stack_down_to(GUEST_STACK_BASE - 4).unwrap_err(),
            ArenaError::StackOverflow(GUEST_STACK_BASE - 4)
        );
    }

    #[test]
    fn write_iat_u32_patches_readonly_page_and_restores_protection() {
        let mut arena = GuestArena::new().unwrap();
        let ro_addr = 0x0040_3000;
        arena.commit_range(ro_addr, GUEST_PAGE_SIZE as usize, false).unwrap();

        assert_eq!(
            arena.write_u32(ro_addr, 0x1122_3344).unwrap_err(),
            ArenaError::ReadOnly(ro_addr)
        );
        arena.write_iat_u32(ro_addr, 0x7000_0008).unwrap();
        assert_eq!(arena.read_u32(ro_addr).unwrap(), 0x7000_0008);
        assert_eq!(
            arena.write_u32(ro_addr, 0x9999_9999).unwrap_err(),
            ArenaError::ReadOnly(ro_addr)
        );
    }
}
