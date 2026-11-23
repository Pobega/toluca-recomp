pub type GuestAddr = u32;

pub const GUEST_ARENA_SIZE: u64 = 1u64 << 32;
pub const GUEST_PAGE_SIZE: u32 = 0x1000;

pub const NULL_GUARD_END: GuestAddr = 0x0001_0000;
pub const DEFAULT_IMAGE_BASE: GuestAddr = 0x0040_0000;
pub const GUEST_HEAP_START: GuestAddr = 0x1000_0000;
pub const GUEST_HEAP_END: GuestAddr = 0x6000_0000;
pub const GUEST_THUNK_BASE: GuestAddr = 0x7000_0000;
pub const GUEST_THUNK_END: GuestAddr = 0x7100_0000;
pub const GUEST_TEB_ADDR: GuestAddr = 0x7E00_0000;
pub const GUEST_PEB_ADDR: GuestAddr = 0x7E00_2000;
pub const GUEST_STACK_BASE: GuestAddr = 0x7F00_0000;
pub const GUEST_STACK_SIZE: u32 = 0x0080_0000;
pub const GUEST_MAIN_STACK_TOP: GuestAddr = 0x7F7F_FFF0;

pub const fn align_up_u32(value: u32, alignment: u32) -> Option<u32> {
    if alignment == 0 {
        return None;
    }
    let remainder = value % alignment;
    if remainder == 0 {
        Some(value)
    } else {
        value.checked_add(alignment - remainder)
    }
}

pub const fn align_down_u32(value: u32, alignment: u32) -> u32 {
    if alignment == 0 {
        return value;
    }
    value - (value % alignment)
}

pub const fn is_page_aligned(addr: GuestAddr) -> bool {
    (addr & (GUEST_PAGE_SIZE - 1)) == 0
}

pub const fn is_thunk_addr(addr: GuestAddr) -> bool {
    addr >= GUEST_THUNK_BASE && addr < GUEST_THUNK_END
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_helpers() {
        assert_eq!(align_up_u32(0, 4096), Some(0));
        assert_eq!(align_up_u32(1, 4096), Some(4096));
        assert_eq!(align_up_u32(4096, 4096), Some(4096));
        assert_eq!(align_up_u32(4097, 4096), Some(8192));
        assert_eq!(align_up_u32(u32::MAX, 16), None);
        assert_eq!(align_up_u32(16, 0), None);

        assert_eq!(align_down_u32(0, 4096), 0);
        assert_eq!(align_down_u32(4095, 4096), 0);
        assert_eq!(align_down_u32(4096, 4096), 4096);
        assert_eq!(align_down_u32(8191, 4096), 4096);
        assert_eq!(align_down_u32(123, 0), 123);

        assert!(is_page_aligned(0));
        assert!(is_page_aligned(DEFAULT_IMAGE_BASE));
        assert!(!is_page_aligned(DEFAULT_IMAGE_BASE + 1));
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn address_space_invariants() {
        assert_eq!(GUEST_ARENA_SIZE, 0x1_0000_0000);
        assert!(NULL_GUARD_END < DEFAULT_IMAGE_BASE);
        assert!(DEFAULT_IMAGE_BASE < GUEST_HEAP_START);
        assert!(GUEST_HEAP_START < GUEST_HEAP_END);
        assert!(GUEST_HEAP_END <= GUEST_THUNK_BASE);
        assert!(GUEST_THUNK_BASE < GUEST_THUNK_END);
        assert!(GUEST_THUNK_END <= GUEST_TEB_ADDR);
        assert!(GUEST_TEB_ADDR < GUEST_PEB_ADDR);
        assert!(GUEST_PEB_ADDR < GUEST_STACK_BASE);
        assert!(GUEST_MAIN_STACK_TOP >= GUEST_STACK_BASE);
        assert!(GUEST_MAIN_STACK_TOP < GUEST_STACK_BASE + GUEST_STACK_SIZE);
        assert_eq!(GUEST_MAIN_STACK_TOP % 16, 0);

        assert!(is_thunk_addr(GUEST_THUNK_BASE));
        assert!(is_thunk_addr(GUEST_THUNK_END - 1));
        assert!(!is_thunk_addr(GUEST_THUNK_END));
        assert!(!is_thunk_addr(GUEST_THUNK_BASE - 1));
    }
}
