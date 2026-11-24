use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

const MAX_WRITE_WATCH_SLOTS: usize = 64;
const FALLBACK_PAGE_SIZE: usize = 4096;

struct WatchSlot {
    start: AtomicUsize,
    end: AtomicUsize,
    dirty: AtomicBool,
}

impl WatchSlot {
    const fn new() -> Self {
        Self {
            start: AtomicUsize::new(0),
            end: AtomicUsize::new(0),
            dirty: AtomicBool::new(false),
        }
    }
}

static WATCH_SLOTS: [WatchSlot; MAX_WRITE_WATCH_SLOTS] =
    [const { WatchSlot::new() }; MAX_WRITE_WATCH_SLOTS];
static WATCH_LOCK: Mutex<()> = Mutex::new(());
static PREV_SA_SIGACTION: AtomicUsize = AtomicUsize::new(0);
static PREV_SA_FLAGS: AtomicI32 = AtomicI32::new(0);

pub fn host_page_size() -> usize {
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();
    *PAGE_SIZE.get_or_init(|| {
        let raw = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if raw > 0 {
            raw as usize
        } else {
            FALLBACK_PAGE_SIZE
        }
    })
}

#[cfg(target_os = "linux")]
extern "C" fn write_watch_sigsegv_handler(
    sig: libc::c_int,
    info: *mut libc::siginfo_t,
    ucontext: *mut c_void,
) {
    let fault_addr = if info.is_null() {
        0usize
    } else {
        unsafe { (*info).si_addr() as usize }
    };
    if fault_addr != 0 {
        for slot in &WATCH_SLOTS {
            let start = slot.start.load(Ordering::Acquire);
            if start == 0 {
                continue;
            }
            let end = slot.end.load(Ordering::Acquire);
            if fault_addr >= start && fault_addr < end {
                slot.dirty.store(true, Ordering::Release);
                unsafe {
                    libc::mprotect(
                        start as *mut c_void,
                        end - start,
                        libc::PROT_READ | libc::PROT_WRITE,
                    );
                }
                return;
            }
        }
    }

    let prev_fn = PREV_SA_SIGACTION.load(Ordering::Acquire);
    let prev_flags = PREV_SA_FLAGS.load(Ordering::Acquire);
    if (prev_flags & libc::SA_SIGINFO) != 0 && prev_fn != 0 {
        let handler: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut c_void) =
            unsafe { std::mem::transmute(prev_fn as *const ()) };
        handler(sig, info, ucontext);
        return;
    }
    if prev_fn != 0 && prev_fn != libc::SIG_DFL && prev_fn != libc::SIG_IGN {
        let handler: extern "C" fn(libc::c_int) =
            unsafe { std::mem::transmute(prev_fn as *const ()) };
        handler(sig);
        return;
    }

    let mut dfl: libc::sigaction = unsafe { std::mem::zeroed() };
    dfl.sa_sigaction = libc::SIG_DFL;
    unsafe {
        libc::sigemptyset(&raw mut dfl.sa_mask);
        libc::sigaction(libc::SIGSEGV, &dfl, std::ptr::null_mut());
    }
}

#[cfg(target_os = "linux")]
fn ensure_handler_installed() {
    let our_addr = write_watch_sigsegv_handler as *const () as usize;
    let mut cur: libc::sigaction = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::sigaction(libc::SIGSEGV, std::ptr::null(), &mut cur) };
    if rc == 0 && cur.sa_sigaction == our_addr {
        return;
    }

    let mut next: libc::sigaction = unsafe { std::mem::zeroed() };
    next.sa_sigaction = our_addr;
    next.sa_flags = libc::SA_SIGINFO;
    let mut prev: libc::sigaction = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&raw mut next.sa_mask);
        if libc::sigaction(libc::SIGSEGV, &next, &mut prev) == 0 && prev.sa_sigaction != our_addr {
            PREV_SA_FLAGS.store(prev.sa_flags, Ordering::Release);
            PREV_SA_SIGACTION.store(prev.sa_sigaction, Ordering::Release);
        }
    }
}

pub fn arm(ptr: *mut u8, len: usize) -> bool {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (ptr, len);
        false
    }
    #[cfg(target_os = "linux")]
    {
        if ptr.is_null() || len == 0 {
            return false;
        }
        let page_size = host_page_size();
        let start = ptr as usize;
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        if !start.is_multiple_of(page_size) || !len.is_multiple_of(page_size) {
            return false;
        }

        let _guard = WATCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        ensure_handler_installed();

        let mut target_idx = None;
        let mut free_idx = None;
        for (idx, slot) in WATCH_SLOTS.iter().enumerate() {
            let s = slot.start.load(Ordering::Relaxed);
            if s == start && slot.end.load(Ordering::Relaxed) == end {
                target_idx = Some(idx);
                break;
            }
            if s == 0 && free_idx.is_none() {
                free_idx = Some(idx);
            }
        }
        let Some(idx) = target_idx.or(free_idx) else {
            return false;
        };

        let slot = &WATCH_SLOTS[idx];
        slot.end.store(end, Ordering::Release);
        slot.dirty.store(false, Ordering::Release);
        slot.start.store(start, Ordering::Release);

        let rc = unsafe { libc::mprotect(start as *mut c_void, len, libc::PROT_READ) };
        if rc != 0 {
            slot.start.store(0, Ordering::Release);
            slot.end.store(0, Ordering::Release);
            return false;
        }
        true
    }
}

pub fn is_dirty(ptr: *const u8, len: usize) -> Option<bool> {
    if ptr.is_null() || len == 0 {
        return None;
    }
    let start = ptr as usize;
    let end = start.checked_add(len)?;
    for slot in &WATCH_SLOTS {
        if slot.start.load(Ordering::Acquire) == start && slot.end.load(Ordering::Acquire) == end {
            return Some(slot.dirty.load(Ordering::Acquire));
        }
    }
    None
}

pub fn remove(ptr: *mut u8, len: usize) {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (ptr, len);
    }
    #[cfg(target_os = "linux")]
    {
        if ptr.is_null() || len == 0 {
            return;
        }
        let start = ptr as usize;
        let Some(end) = start.checked_add(len) else {
            return;
        };
        let _guard = WATCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for slot in &WATCH_SLOTS {
            if slot.start.load(Ordering::Relaxed) == start && slot.end.load(Ordering::Relaxed) == end
            {
                unsafe {
                    libc::mprotect(
                        start as *mut c_void,
                        len,
                        libc::PROT_READ | libc::PROT_WRITE,
                    );
                }
                slot.dirty.store(false, Ordering::Release);
                slot.start.store(0, Ordering::Release);
                slot.end.store(0, Ordering::Release);
                return;
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn write_watch_detects_faults_and_rearms_cleanly() {
        let page_size = host_page_size();
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                page_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(raw, libc::MAP_FAILED);
        let ptr = raw.cast::<u8>();

        unsafe {
            ptr.write_volatile(0x11);
        }
        assert!(!arm(unsafe { ptr.add(1) }, page_size));
        assert!(arm(ptr, page_size));
        assert_eq!(is_dirty(ptr, page_size), Some(false));

        // Reads must not trip the write-watch.
        let read_back = unsafe { ptr.read_volatile() };
        assert_eq!(read_back, 0x11);
        assert_eq!(is_dirty(ptr, page_size), Some(false));

        // Raw write triggers SIGSEGV, marks dirty, and restores PROT_READ | PROT_WRITE.
        unsafe {
            ptr.add(17).write_volatile(0x5A);
        }
        assert_eq!(is_dirty(ptr, page_size), Some(true));
        assert_eq!(unsafe { ptr.add(17).read_volatile() }, 0x5A);

        // Re-arming resets dirty flag cleanly.
        assert!(arm(ptr, page_size));
        assert_eq!(is_dirty(ptr, page_size), Some(false));

        remove(ptr, page_size);
        assert_eq!(is_dirty(ptr, page_size), None);
        unsafe {
            ptr.add(17).write_volatile(0x7C);
            assert_eq!(ptr.add(17).read_volatile(), 0x7C);
            libc::munmap(raw, page_size);
        }
    }
}
