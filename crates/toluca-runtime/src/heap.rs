use std::collections::BTreeMap;
use std::fmt;

use toluca_core::GuestAddr;
use toluca_core::memory::{
    GUEST_HEAP_END, GUEST_HEAP_START, GUEST_PAGE_SIZE, align_down_u32, align_up_u32,
};

use crate::arena::{ArenaError, GuestArena};

pub const HEAP_REGION_START: GuestAddr = GUEST_HEAP_START;
pub const HEAP_REGION_END: GuestAddr = 0x4000_0000;
pub const VIRTUAL_ALLOC_START: GuestAddr = HEAP_REGION_END;
pub const VIRTUAL_ALLOC_END: GuestAddr = GUEST_HEAP_END;

pub const HEAP_ALIGNMENT: usize = 16;
pub const HEAP_COMMIT_CHUNK: u32 = 0x0001_0000;
pub const HEAP_SHRINK_THRESHOLD: u32 = 0x0002_0000;

pub const VIRTUAL_ALLOC_GRANULARITY: u32 = 0x0001_0000;

pub const MEM_COMMIT: u32 = 0x0000_1000;
pub const MEM_RESERVE: u32 = 0x0000_2000;
pub const MEM_DECOMMIT: u32 = 0x0000_4000;
pub const MEM_RELEASE: u32 = 0x0000_8000;

pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_EXECUTE_READ: u32 = 0x20;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeapError {
    OutOfMemory,
    InvalidPointer(GuestAddr),
    InvalidSize(usize),
    InvalidFlags(u32),
    InvalidProtection(u32),
    AddressUnavailable(GuestAddr),
    Arena(ArenaError),
}

impl fmt::Display for HeapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfMemory => write!(f, "guest allocator exhausted virtual address space"),
            Self::InvalidPointer(addr) => write!(f, "invalid guest allocation pointer 0x{addr:08x}"),
            Self::InvalidSize(size) => write!(f, "invalid allocation size {size}"),
            Self::InvalidFlags(flags) => write!(f, "invalid virtual memory flags 0x{flags:08x}"),
            Self::InvalidProtection(prot) => {
                write!(f, "unsupported page protection 0x{prot:08x}")
            }
            Self::AddressUnavailable(addr) => {
                write!(f, "requested virtual address 0x{addr:08x} is unavailable")
            }
            Self::Arena(err) => write!(f, "arena error: {err}"),
        }
    }
}

impl std::error::Error for HeapError {}

impl From<ArenaError> for HeapError {
    fn from(err: ArenaError) -> Self {
        Self::Arena(err)
    }
}

pub struct GuestHeap {
    region_start: GuestAddr,
    region_end: GuestAddr,
    bump_cursor: GuestAddr,
    committed_end: GuestAddr,
    allocated: BTreeMap<GuestAddr, u32>,
    free_by_addr: BTreeMap<GuestAddr, u32>,
}

impl Default for GuestHeap {
    fn default() -> Self {
        Self::new()
    }
}

impl GuestHeap {
    pub fn new() -> Self {
        Self {
            region_start: HEAP_REGION_START,
            region_end: HEAP_REGION_END,
            bump_cursor: HEAP_REGION_START,
            committed_end: HEAP_REGION_START,
            allocated: BTreeMap::new(),
            free_by_addr: BTreeMap::new(),
        }
    }

    pub fn committed_bytes(&self) -> u32 {
        self.committed_end - self.region_start
    }

    pub fn block_size(&self, ptr: GuestAddr) -> Option<usize> {
        self.allocated.get(&ptr).map(|&sz| sz as usize)
    }

    pub fn alloc(
        &mut self,
        arena: &mut GuestArena,
        size: usize,
        zeroed: bool,
    ) -> Result<GuestAddr, HeapError> {
        let needed = align_alloc_size(size)?;

        if let Some((block_addr, block_size)) = self.find_free_block(needed) {
            self.free_by_addr.remove(&block_addr);
            if block_size > needed {
                self.free_by_addr
                    .insert(block_addr + needed, block_size - needed);
            }
            self.allocated.insert(block_addr, needed);
            if zeroed {
                arena.zero_range(block_addr, needed as usize)?;
            }
            return Ok(block_addr);
        }

        let ptr = self.bump_cursor;
        let new_bump = ptr.checked_add(needed).ok_or(HeapError::OutOfMemory)?;
        if new_bump > self.region_end {
            return Err(HeapError::OutOfMemory);
        }

        self.ensure_committed(arena, new_bump)?;
        self.bump_cursor = new_bump;
        self.allocated.insert(ptr, needed);

        if zeroed {
            arena.zero_range(ptr, needed as usize)?;
        }
        Ok(ptr)
    }

    pub fn realloc(
        &mut self,
        arena: &mut GuestArena,
        ptr: GuestAddr,
        new_size: usize,
        zeroed: bool,
    ) -> Result<GuestAddr, HeapError> {
        if ptr == 0 {
            return self.alloc(arena, new_size, zeroed);
        }
        if new_size == 0 {
            self.free(arena, ptr)?;
            return Ok(0);
        }

        let old_size = *self
            .allocated
            .get(&ptr)
            .ok_or(HeapError::InvalidPointer(ptr))?;
        let new_aligned = align_alloc_size(new_size)?;

        if new_aligned == old_size {
            return Ok(ptr);
        }

        if new_aligned < old_size {
            self.allocated.insert(ptr, new_aligned);
            self.release_range(arena, ptr + new_aligned, old_size - new_aligned)?;
            return Ok(ptr);
        }

        let block_end = ptr + old_size;
        if block_end == self.bump_cursor {
            let new_bump = ptr.checked_add(new_aligned).ok_or(HeapError::OutOfMemory)?;
            if new_bump <= self.region_end {
                self.ensure_committed(arena, new_bump)?;
                self.bump_cursor = new_bump;
                self.allocated.insert(ptr, new_aligned);
                if zeroed {
                    arena.zero_range(block_end, (new_aligned - old_size) as usize)?;
                }
                return Ok(ptr);
            }
        }

        if let Some(&next_size) = self.free_by_addr.get(&block_end) {
            let combined = old_size.saturating_add(next_size);
            if combined >= new_aligned {
                self.free_by_addr.remove(&block_end);
                if combined > new_aligned {
                    self.free_by_addr
                        .insert(ptr + new_aligned, combined - new_aligned);
                }
                self.allocated.insert(ptr, new_aligned);
                if zeroed {
                    arena.zero_range(block_end, (new_aligned - old_size) as usize)?;
                }
                return Ok(ptr);
            }
        }

        let new_ptr = self.alloc(arena, new_size, zeroed)?;
        arena.copy_bytes(new_ptr, ptr, old_size as usize)?;
        self.free(arena, ptr)?;
        Ok(new_ptr)
    }

    pub fn free(&mut self, arena: &mut GuestArena, ptr: GuestAddr) -> Result<(), HeapError> {
        let size = self
            .allocated
            .remove(&ptr)
            .ok_or(HeapError::InvalidPointer(ptr))?;
        self.release_range(arena, ptr, size)
    }

    fn find_free_block(&self, needed: u32) -> Option<(GuestAddr, u32)> {
        let mut first_fit = None;
        for (&addr, &size) in &self.free_by_addr {
            if size == needed {
                return Some((addr, size));
            }
            if size > needed && first_fit.is_none() {
                first_fit = Some((addr, size));
            }
        }
        first_fit
    }

    fn ensure_committed(&mut self, arena: &mut GuestArena, needed_end: GuestAddr) -> Result<(), HeapError> {
        if needed_end <= self.committed_end {
            return Ok(());
        }
        let target = align_up_u32(needed_end, HEAP_COMMIT_CHUNK)
            .unwrap_or(self.region_end)
            .min(self.region_end);
        let len = (target - self.committed_end) as usize;
        arena.commit_range(self.committed_end, len, true)?;
        self.committed_end = target;
        Ok(())
    }

    fn release_range(
        &mut self,
        arena: &mut GuestArena,
        addr: GuestAddr,
        size: u32,
    ) -> Result<(), HeapError> {
        let mut cur_addr = addr;
        let mut cur_size = size;

        if let Some((&prev_addr, &prev_size)) = self.free_by_addr.range(..cur_addr).next_back() {
            if prev_addr + prev_size == cur_addr {
                self.free_by_addr.remove(&prev_addr);
                cur_addr = prev_addr;
                cur_size += prev_size;
            }
        }

        if let Some((&next_addr, &next_size)) = self.free_by_addr.range(cur_addr..).next() {
            if cur_addr + cur_size == next_addr {
                self.free_by_addr.remove(&next_addr);
                cur_size += next_size;
            }
        }

        if cur_addr + cur_size == self.bump_cursor {
            self.bump_cursor = cur_addr;
            self.shrink_if_excess(arena)?;
        } else {
            self.free_by_addr.insert(cur_addr, cur_size);
        }

        Ok(())
    }

    fn shrink_if_excess(&mut self, arena: &mut GuestArena) -> Result<(), HeapError> {
        let trailing_free = self.committed_end - self.bump_cursor;
        if trailing_free < HEAP_SHRINK_THRESHOLD {
            return Ok(());
        }
        let new_commit = align_up_u32(self.bump_cursor, HEAP_COMMIT_CHUNK)
            .unwrap_or(self.committed_end)
            .max(self.region_start);
        if new_commit < self.committed_end {
            let decommit_len = (self.committed_end - new_commit) as usize;
            arena.decommit_range(new_commit, decommit_len)?;
            self.committed_end = new_commit;
        }
        Ok(())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct VirtualReservation {
    base: GuestAddr,
    size: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PageAccess {
    NoAccess,
    ReadOnly,
    ReadWrite,
}

pub struct VirtualMemoryManager {
    region_start: GuestAddr,
    region_end: GuestAddr,
    reservations: BTreeMap<GuestAddr, VirtualReservation>,
}

impl Default for VirtualMemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualMemoryManager {
    pub fn new() -> Self {
        Self {
            region_start: VIRTUAL_ALLOC_START,
            region_end: VIRTUAL_ALLOC_END,
            reservations: BTreeMap::new(),
        }
    }

    pub fn reservation_count(&self) -> usize {
        self.reservations.len()
    }

    pub fn virtual_alloc(
        &mut self,
        arena: &mut GuestArena,
        addr: Option<GuestAddr>,
        size: usize,
        alloc_type: u32,
        protect: u32,
    ) -> Result<GuestAddr, HeapError> {
        if size == 0 {
            return Err(HeapError::InvalidSize(size));
        }

        let valid_type_mask = MEM_RESERVE | MEM_COMMIT;
        if (alloc_type & valid_type_mask) == 0 || (alloc_type & !valid_type_mask) != 0 {
            return Err(HeapError::InvalidFlags(alloc_type));
        }

        let access = decode_protection(protect)?;
        let requested_addr = addr.filter(|&a| a != 0);

        if (alloc_type & MEM_RESERVE) != 0 {
            let size_u32 = u32::try_from(size).map_err(|_| HeapError::OutOfMemory)?;
            let aligned_size = align_up_u32(size_u32, VIRTUAL_ALLOC_GRANULARITY)
                .ok_or(HeapError::OutOfMemory)?;

            let base = match requested_addr {
                Some(req) => {
                    let aligned_base = align_down_u32(req, VIRTUAL_ALLOC_GRANULARITY);
                    if !self.is_free_range(aligned_base, aligned_size) {
                        return Err(HeapError::AddressUnavailable(req));
                    }
                    aligned_base
                }
                None => self
                    .find_free_region(aligned_size)
                    .ok_or(HeapError::OutOfMemory)?,
            };

            self.reservations
                .insert(base, VirtualReservation { base, size: aligned_size });

            if (alloc_type & MEM_COMMIT) != 0 {
                apply_commit_protection(arena, base, aligned_size, access)?;
            }
            return Ok(base);
        }

        let req = requested_addr.ok_or(HeapError::InvalidPointer(0))?;
        let res = self
            .find_reservation_containing(req)
            .ok_or(HeapError::InvalidPointer(req))?;

        let size_u32 = u32::try_from(size).map_err(|_| HeapError::InvalidSize(size))?;
        let commit_start = align_down_u32(req, GUEST_PAGE_SIZE);
        let raw_end = req
            .checked_add(size_u32)
            .ok_or(HeapError::InvalidSize(size))?;
        let commit_end = align_up_u32(raw_end, GUEST_PAGE_SIZE).ok_or(HeapError::InvalidSize(size))?;

        if commit_end > res.base + res.size {
            return Err(HeapError::AddressUnavailable(req));
        }

        apply_commit_protection(arena, commit_start, commit_end - commit_start, access)?;
        Ok(commit_start)
    }

    pub fn virtual_free(
        &mut self,
        arena: &mut GuestArena,
        addr: GuestAddr,
        size: usize,
        free_type: u32,
    ) -> Result<(), HeapError> {
        match free_type {
            MEM_RELEASE => {
                if size != 0 {
                    return Err(HeapError::InvalidSize(size));
                }
                let res = self
                    .reservations
                    .remove(&addr)
                    .ok_or(HeapError::InvalidPointer(addr))?;
                arena.decommit_range(res.base, res.size as usize)?;
                Ok(())
            }
            MEM_DECOMMIT => {
                let res = self
                    .find_reservation_containing(addr)
                    .ok_or(HeapError::InvalidPointer(addr))?;
                if size == 0 {
                    if addr != res.base {
                        return Err(HeapError::InvalidPointer(addr));
                    }
                    arena.decommit_range(res.base, res.size as usize)?;
                    return Ok(());
                }

                let size_u32 = u32::try_from(size).map_err(|_| HeapError::InvalidSize(size))?;
                let start = align_down_u32(addr, GUEST_PAGE_SIZE);
                let raw_end = addr
                    .checked_add(size_u32)
                    .ok_or(HeapError::InvalidSize(size))?;
                let end = align_up_u32(raw_end, GUEST_PAGE_SIZE)
                    .ok_or(HeapError::InvalidSize(size))?;
                if end > res.base + res.size {
                    return Err(HeapError::AddressUnavailable(addr));
                }
                arena.decommit_range(start, (end - start) as usize)?;
                Ok(())
            }
            _ => Err(HeapError::InvalidFlags(free_type)),
        }
    }

    fn find_reservation_containing(&self, addr: GuestAddr) -> Option<VirtualReservation> {
        let (_, &res) = self.reservations.range(..=addr).next_back()?;
        if addr >= res.base && addr < res.base + res.size {
            Some(res)
        } else {
            None
        }
    }

    fn is_free_range(&self, base: GuestAddr, size: u32) -> bool {
        let Some(end) = base.checked_add(size) else {
            return false;
        };
        if base < self.region_start || end > self.region_end {
            return false;
        }
        if let Some((_, &prev)) = self.reservations.range(..=base).next_back() {
            if prev.base + prev.size > base {
                return false;
            }
        }
        if let Some((_, &next)) = self.reservations.range(base..).next() {
            if next.base < end {
                return false;
            }
        }
        true
    }

    fn find_free_region(&self, size: u32) -> Option<GuestAddr> {
        let mut cursor = self.region_start;
        for &res in self.reservations.values() {
            if res.base >= cursor && res.base - cursor >= size {
                return Some(cursor);
            }
            cursor = cursor.max(res.base + res.size);
        }
        if self.region_end - cursor >= size {
            Some(cursor)
        } else {
            None
        }
    }
}

fn align_alloc_size(size: usize) -> Result<u32, HeapError> {
    let min_size = size.max(1);
    let aligned = min_size
        .checked_add(HEAP_ALIGNMENT - 1)
        .map(|n| n & !(HEAP_ALIGNMENT - 1))
        .ok_or(HeapError::OutOfMemory)?;
    u32::try_from(aligned).map_err(|_| HeapError::OutOfMemory)
}

fn decode_protection(protect: u32) -> Result<PageAccess, HeapError> {
    match protect {
        PAGE_NOACCESS => Ok(PageAccess::NoAccess),
        PAGE_READONLY | PAGE_EXECUTE_READ => Ok(PageAccess::ReadOnly),
        PAGE_READWRITE | PAGE_EXECUTE_READWRITE => Ok(PageAccess::ReadWrite),
        _ => Err(HeapError::InvalidProtection(protect)),
    }
}

fn apply_commit_protection(
    arena: &mut GuestArena,
    addr: GuestAddr,
    len: u32,
    access: PageAccess,
) -> Result<(), HeapError> {
    match access {
        PageAccess::NoAccess => arena.decommit_range(addr, len as usize)?,
        PageAccess::ReadOnly => arena.commit_range(addr, len as usize, false)?,
        PageAccess::ReadWrite => arena.commit_range(addr, len as usize, true)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_alloc_zeroed_and_alignment() {
        let mut arena = GuestArena::new().unwrap();
        let mut heap = GuestHeap::new();

        let p1 = heap.alloc(&mut arena, 1, false).unwrap();
        let p2 = heap.alloc(&mut arena, 24, true).unwrap();
        assert_eq!(p1 % HEAP_ALIGNMENT as u32, 0);
        assert_eq!(p2 % HEAP_ALIGNMENT as u32, 0);
        assert_eq!(heap.block_size(p1), Some(16));
        assert_eq!(heap.block_size(p2), Some(32));

        for off in 0..32 {
            assert_eq!(arena.read_u8(p2 + off).unwrap(), 0);
        }
        assert_eq!(heap.committed_bytes(), HEAP_COMMIT_CHUNK);
    }

    #[test]
    fn heap_free_coalescing_and_shrink() {
        let mut arena = GuestArena::new().unwrap();
        let mut heap = GuestHeap::new();

        let a = heap.alloc(&mut arena, 0x1_0000, false).unwrap();
        let b = heap.alloc(&mut arena, 0x1_0000, false).unwrap();
        let c = heap.alloc(&mut arena, 0x1_0000, false).unwrap();
        let d = heap.alloc(&mut arena, 0x1_0000, false).unwrap();
        assert!(heap.committed_bytes() >= 0x4_0000);

        // Free a and c, then free b to coalesce all three into one 0x3_0000 block
        heap.free(&mut arena, a).unwrap();
        heap.free(&mut arena, c).unwrap();
        heap.free(&mut arena, b).unwrap();

        let merged = heap.alloc(&mut arena, 0x3_0000, false).unwrap();
        assert_eq!(merged, a);

        // Freeing trailing blocks past shrink threshold decommits pages back to host
        heap.free(&mut arena, d).unwrap();
        heap.free(&mut arena, merged).unwrap();
        assert_eq!(heap.committed_bytes(), 0);
        assert!(!arena.is_range_committed(HEAP_REGION_START, HEAP_COMMIT_CHUNK as usize));
    }

    #[test]
    fn heap_realloc_all_paths() {
        let mut arena = GuestArena::new().unwrap();
        let mut heap = GuestHeap::new();

        // ptr == 0 acts as alloc
        let p1 = heap.realloc(&mut arena, 0, 32, false).unwrap();
        arena.write_u32(p1, 0xCAFE_BABE).unwrap();

        // Grow at bump frontier in-place with zeroed extension
        let p1_grown = heap.realloc(&mut arena, p1, 64, true).unwrap();
        assert_eq!(p1_grown, p1);
        assert_eq!(arena.read_u32(p1_grown).unwrap(), 0xCAFE_BABE);
        assert_eq!(arena.read_u32(p1_grown + 32).unwrap(), 0);

        // Shrink in-place
        let p2 = heap.alloc(&mut arena, 32, false).unwrap();
        let p3 = heap.alloc(&mut arena, 32, false).unwrap();
        let p1_shrunk = heap.realloc(&mut arena, p1_grown, 16, false).unwrap();
        assert_eq!(p1_shrunk, p1_grown);
        assert_eq!(heap.block_size(p1_shrunk), Some(16));

        // Grow into adjacent freed block (p2)
        heap.free(&mut arena, p2).unwrap();
        let p1_into_p2 = heap.realloc(&mut arena, p1_shrunk, 80, true).unwrap();
        assert_eq!(p1_into_p2, p1_shrunk);
        assert_eq!(arena.read_u32(p1_into_p2).unwrap(), 0xCAFE_BABE);
        assert_eq!(arena.read_u32(p1_into_p2 + 48).unwrap(), 0);

        // Fallback move when blocked by p3
        let p1_moved = heap.realloc(&mut arena, p1_into_p2, 256, true).unwrap();
        assert_ne!(p1_moved, p1_into_p2);
        assert_eq!(arena.read_u32(p1_moved).unwrap(), 0xCAFE_BABE);
        assert!(heap.block_size(p1_into_p2).is_none());

        // new_size == 0 frees the block
        assert_eq!(heap.realloc(&mut arena, p1_moved, 0, false).unwrap(), 0);
        assert!(heap.block_size(p1_moved).is_none());
        heap.free(&mut arena, p3).unwrap();
    }

    #[test]
    fn virtual_alloc_reserve_commit_decommit_release() {
        let mut arena = GuestArena::new().unwrap();
        let mut vmm = VirtualMemoryManager::new();

        // Reserve 128 KiB without committing
        let base = vmm
            .virtual_alloc(&mut arena, None, 0x2_0000, MEM_RESERVE, PAGE_READWRITE)
            .unwrap();
        assert_eq!(base % VIRTUAL_ALLOC_GRANULARITY, 0);
        assert!(!arena.is_range_committed(base, 0x1000));

        // Commit second 64 KiB page range
        let commit_addr = vmm
            .virtual_alloc(
                &mut arena,
                Some(base + 0x1_0000),
                0x1_0000,
                MEM_COMMIT,
                PAGE_READWRITE,
            )
            .unwrap();
        assert_eq!(commit_addr, base + 0x1_0000);
        arena.write_u32(commit_addr, 0x1337_BEEF).unwrap();
        assert_eq!(arena.read_u32(commit_addr).unwrap(), 0x1337_BEEF);

        // Decommit immediately unmaps in arena while keeping reservation
        vmm.virtual_free(&mut arena, commit_addr, 0x1_0000, MEM_DECOMMIT)
            .unwrap();
        assert!(!arena.is_range_committed(commit_addr, 0x1_0000));
        assert_eq!(vmm.reservation_count(), 1);

        // Preferred-address reserve collision fails cleanly
        assert_eq!(
            vmm.virtual_alloc(&mut arena, Some(base), 0x1_0000, MEM_RESERVE, PAGE_READWRITE)
                .unwrap_err(),
            HeapError::AddressUnavailable(base)
        );

        // Release frees reservation and allows re-reserving at the same preferred address
        vmm.virtual_free(&mut arena, base, 0, MEM_RELEASE).unwrap();
        assert_eq!(vmm.reservation_count(), 0);

        let reused = vmm
            .virtual_alloc(
                &mut arena,
                Some(base),
                0x1_0000,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READONLY,
            )
            .unwrap();
        assert_eq!(reused, base);
        assert_eq!(arena.read_u32(reused).unwrap(), 0);
        assert_eq!(
            arena.write_u32(reused, 1).unwrap_err(),
            ArenaError::ReadOnly(reused)
        );

        vmm.virtual_free(&mut arena, reused, 0, MEM_RELEASE).unwrap();
    }
}
