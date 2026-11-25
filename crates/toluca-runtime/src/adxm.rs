use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};

use toluca_core::GuestAddr;
use toluca_core::pe::PeImage;

const ADXM_LOCK_LEN: usize = 0x45;
const ADXM_UNLOCK_LEN: usize = 0x32;

const ADXM_LOCK_OP_CMP_JNZ_CALL: [u8; 8] = [0x85, 0xC0, 0x75, 0x35, 0x56, 0x57, 0xFF, 0x15];
const ADXM_LOCK_OP_MOV_ESI_PUSH_CALL: [u8; 5] = [0x8B, 0xF0, 0x56, 0xFF, 0x15];
const ADXM_LOCK_OP_MOV_EDI_MOV_EAX: [u8; 3] = [0x8B, 0xF8, 0xA1];
const ADXM_LOCK_OP_PUSH_EAX_ESI_CALL: [u8; 4] = [0x50, 0x56, 0xFF, 0x15];
const ADXM_LOCK_OP_MOV_ECX: [u8; 2] = [0x8B, 0x0D];
const ADXM_LOCK_OP_MOV_MEM_EDI: [u8; 2] = [0x89, 0x3D];
const ADXM_LOCK_OP_PUSH_ECX_CALL: [u8; 3] = [0x51, 0xFF, 0x15];
const ADXM_LOCK_OP_POP_INC_MEM: [u8; 4] = [0x5F, 0x5E, 0xFF, 0x05];

const ADXM_UNLOCK_OP_DEC_MOV: [u8; 2] = [0x48, 0xA3];
const ADXM_UNLOCK_OP_JNZ_PUSH_CALL: [u8; 5] = [0x75, 0x24, 0x56, 0xFF, 0x15];
const ADXM_UNLOCK_OP_MOV_ESI_MOV_EAX: [u8; 3] = [0x8B, 0xF0, 0xA1];
const ADXM_UNLOCK_OP_PUSH_EAX_CALL: [u8; 3] = [0x50, 0xFF, 0x15];
const ADXM_UNLOCK_OP_MOV_ECX: [u8; 2] = [0x8B, 0x0D];
const ADXM_UNLOCK_OP_PUSH_ECX_ESI_CALL: [u8; 4] = [0x51, 0x56, 0xFF, 0x15];
const ADXM_UNLOCK_OP_EPILOGUE: [u8; 2] = [0x5E, 0xC3];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AdxmLockMatch {
    pub lock_va: GuestAddr,
    pub unlock_va: GuestAddr,
    pub lock_depth_va: GuestAddr,
}

struct AdxmLockCandidate {
    lock_va: GuestAddr,
    lock_depth_va: GuestAddr,
    iat_get_cur: u32,
    iat_set_prio: u32,
    spin_handle_va: GuestAddr,
    prev_prio_va: GuestAddr,
}

#[cfg(target_os = "linux")]
const ADXM_CEILING_NICE: i32 = -19;

#[derive(Default)]
struct AdxmLockState {
    owner_tid: u64,
    depth: u32,
    saved_nice: Option<i32>,
}

pub struct AdxmMutexShim {
    lock_va: GuestAddr,
    unlock_va: GuestAddr,
    lock_depth_va: GuestAddr,
    state: Mutex<AdxmLockState>,
    cv: Condvar,
}

impl AdxmMutexShim {
    pub fn new(matched: AdxmLockMatch) -> Self {
        Self {
            lock_va: matched.lock_va,
            unlock_va: matched.unlock_va,
            lock_depth_va: matched.lock_depth_va,
            state: Mutex::new(AdxmLockState::default()),
            cv: Condvar::new(),
        }
    }

    pub fn lock_va(&self) -> GuestAddr {
        self.lock_va
    }

    pub fn unlock_va(&self) -> GuestAddr {
        self.unlock_va
    }

    pub fn lock_depth_va(&self) -> GuestAddr {
        self.lock_depth_va
    }

    pub fn lock(&self, tid: u64, arena_base: usize, shutdown: &AtomicBool) -> u32 {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while guard.owner_tid != 0 && guard.owner_tid != tid && !shutdown.load(Ordering::Acquire) {
            guard = self.cv.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
        if shutdown.load(Ordering::Acquire) && tid != 1 {
            return 0;
        }
        let is_outer = guard.depth == 0;
        guard.owner_tid = tid;
        guard.depth = guard.depth.saturating_add(1);
        let depth = guard.depth;
        write_guest_depth(arena_base, self.lock_depth_va, depth);
        if is_outer {
            guard.saved_nice = boost_calling_thread_nice();
        }
        depth
    }

    pub fn unlock(&self, tid: u64, arena_base: usize) -> u32 {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if guard.owner_tid != tid || guard.depth == 0 {
            return 0;
        }
        guard.depth -= 1;
        let depth = guard.depth;
        write_guest_depth(arena_base, self.lock_depth_va, depth);
        let saved = if depth == 0 {
            guard.owner_tid = 0;
            guard.saved_nice.take()
        } else {
            None
        };
        drop(guard);
        if depth == 0 {
            restore_calling_thread_nice(saved);
            self.cv.notify_one();
        }
        depth
    }

    pub fn release_if_owned(&self, tid: u64, arena_base: usize) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if guard.owner_tid == tid {
            guard.depth = 0;
            guard.owner_tid = 0;
            let saved = guard.saved_nice.take();
            write_guest_depth(arena_base, self.lock_depth_va, 0);
            drop(guard);
            restore_calling_thread_nice(saved);
            self.cv.notify_all();
        }
    }

    pub fn notify_all(&self) {
        let _guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.cv.notify_all();
    }
}

#[cfg(target_os = "linux")]
fn current_thread_nice() -> Option<i32> {
    unsafe {
        *libc::__errno_location() = 0;
        let prio = libc::getpriority(libc::PRIO_PROCESS, 0);
        if *libc::__errno_location() != 0 {
            return None;
        }
        Some(prio)
    }
}

#[cfg(target_os = "linux")]
fn boost_calling_thread_nice() -> Option<i32> {
    let prev = current_thread_nice()?;
    if prev <= ADXM_CEILING_NICE {
        return None;
    }
    let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, ADXM_CEILING_NICE) };
    if rc == 0 { Some(prev) } else { None }
}

#[cfg(target_os = "linux")]
fn restore_calling_thread_nice(saved: Option<i32>) {
    if let Some(prev) = saved {
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, prev);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn boost_calling_thread_nice() -> Option<i32> {
    None
}

#[cfg(not(target_os = "linux"))]
fn restore_calling_thread_nice(_saved: Option<i32>) {}

fn write_guest_depth(arena_base: usize, lock_depth_va: GuestAddr, depth: u32) {
    if arena_base == 0 {
        return;
    }
    let bytes = depth.to_le_bytes();
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (arena_base + lock_depth_va as usize) as *mut u8,
            bytes.len(),
        );
    }
}

fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn match_adxm_lock(slice: &[u8], lock_va: GuestAddr) -> Option<AdxmLockCandidate> {
    if slice.len() < ADXM_LOCK_LEN || slice[0x00] != 0xA1 || slice[0x44] != 0xC3 {
        return None;
    }
    if slice[0x05..0x0D] != ADXM_LOCK_OP_CMP_JNZ_CALL
        || slice[0x11..0x16] != ADXM_LOCK_OP_MOV_ESI_PUSH_CALL
        || slice[0x1A..0x1D] != ADXM_LOCK_OP_MOV_EDI_MOV_EAX
        || slice[0x21..0x25] != ADXM_LOCK_OP_PUSH_EAX_ESI_CALL
        || slice[0x29..0x2B] != ADXM_LOCK_OP_MOV_ECX
        || slice[0x2F..0x31] != ADXM_LOCK_OP_MOV_MEM_EDI
        || slice[0x35..0x38] != ADXM_LOCK_OP_PUSH_ECX_CALL
        || slice[0x3C..0x40] != ADXM_LOCK_OP_POP_INC_MEM
    {
        return None;
    }
    let lock_depth_va = read_u32_le(slice, 0x01);
    if read_u32_le(slice, 0x40) != lock_depth_va {
        return None;
    }
    Some(AdxmLockCandidate {
        lock_va,
        lock_depth_va,
        iat_get_cur: read_u32_le(slice, 0x0D),
        iat_set_prio: read_u32_le(slice, 0x25),
        spin_handle_va: read_u32_le(slice, 0x2B),
        prev_prio_va: read_u32_le(slice, 0x31),
    })
}

fn match_adxm_unlock(slice: &[u8], cand: &AdxmLockCandidate) -> bool {
    if slice.len() < ADXM_UNLOCK_LEN || slice[0x00] != 0xA1 || slice[0x30..0x32] != ADXM_UNLOCK_OP_EPILOGUE {
        return false;
    }
    slice[0x05..0x07] == ADXM_UNLOCK_OP_DEC_MOV
        && slice[0x0B..0x10] == ADXM_UNLOCK_OP_JNZ_PUSH_CALL
        && slice[0x14..0x17] == ADXM_UNLOCK_OP_MOV_ESI_MOV_EAX
        && slice[0x1B..0x1E] == ADXM_UNLOCK_OP_PUSH_EAX_CALL
        && slice[0x22..0x24] == ADXM_UNLOCK_OP_MOV_ECX
        && slice[0x28..0x2C] == ADXM_UNLOCK_OP_PUSH_ECX_ESI_CALL
        && read_u32_le(slice, 0x01) == cand.lock_depth_va
        && read_u32_le(slice, 0x07) == cand.lock_depth_va
        && read_u32_le(slice, 0x10) == cand.iat_get_cur
        && read_u32_le(slice, 0x17) == cand.spin_handle_va
        && read_u32_le(slice, 0x24) == cand.prev_prio_va
        && read_u32_le(slice, 0x2C) == cand.iat_set_prio
}

fn detect_adxm_lock_regions(regions: &[(GuestAddr, &[u8])]) -> Option<AdxmLockMatch> {
    for &(base_va, bytes) in regions {
        if bytes.len() < ADXM_LOCK_LEN {
            continue;
        }
        for i in 0..=(bytes.len() - ADXM_LOCK_LEN) {
            let Some(cand) = match_adxm_lock(&bytes[i..], base_va.saturating_add(i as u32)) else {
                continue;
            };
            for &(u_base, u_bytes) in regions {
                if u_bytes.len() < ADXM_UNLOCK_LEN {
                    continue;
                }
                for j in 0..=(u_bytes.len() - ADXM_UNLOCK_LEN) {
                    if match_adxm_unlock(&u_bytes[j..], &cand) {
                        return Some(AdxmLockMatch {
                            lock_va: cand.lock_va,
                            unlock_va: u_base.saturating_add(j as u32),
                            lock_depth_va: cand.lock_depth_va,
                        });
                    }
                }
            }
        }
    }
    None
}

pub fn detect_adxm_lock_in_slice(base_va: GuestAddr, bytes: &[u8]) -> Option<AdxmLockMatch> {
    detect_adxm_lock_regions(&[(base_va, bytes)])
}

pub fn detect_adxm_lock_pair(pe: &PeImage) -> Option<AdxmLockMatch> {
    if std::env::var_os("TOLUCA_ADXM_MUTEX").is_some_and(|v| v == "0") {
        return None;
    }
    let regions: Vec<(GuestAddr, &[u8])> = pe
        .sections
        .iter()
        .filter(|sec| sec.is_executable())
        .map(|sec| {
            (
                pe.image_base.saturating_add(sec.virtual_address),
                sec.raw_bytes(&pe.raw),
            )
        })
        .collect();
    detect_adxm_lock_regions(&regions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn build_synthetic_adxm_lock_bytes(
        lock_depth_va: u32,
        iat_get_cur: u32,
        iat_set_prio: u32,
        spin_handle_va: u32,
        prev_prio_va: u32,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(ADXM_LOCK_LEN);
        buf.push(0xA1);
        buf.extend_from_slice(&lock_depth_va.to_le_bytes());
        buf.extend_from_slice(&ADXM_LOCK_OP_CMP_JNZ_CALL);
        buf.extend_from_slice(&iat_get_cur.to_le_bytes());
        buf.extend_from_slice(&ADXM_LOCK_OP_MOV_ESI_PUSH_CALL);
        buf.extend_from_slice(&0x0060_0008u32.to_le_bytes());
        buf.extend_from_slice(&ADXM_LOCK_OP_MOV_EDI_MOV_EAX);
        buf.extend_from_slice(&0x0070_0010u32.to_le_bytes());
        buf.extend_from_slice(&ADXM_LOCK_OP_PUSH_EAX_ESI_CALL);
        buf.extend_from_slice(&iat_set_prio.to_le_bytes());
        buf.extend_from_slice(&ADXM_LOCK_OP_MOV_ECX);
        buf.extend_from_slice(&spin_handle_va.to_le_bytes());
        buf.extend_from_slice(&ADXM_LOCK_OP_MOV_MEM_EDI);
        buf.extend_from_slice(&prev_prio_va.to_le_bytes());
        buf.extend_from_slice(&ADXM_LOCK_OP_PUSH_ECX_CALL);
        buf.extend_from_slice(&0x0060_0014u32.to_le_bytes());
        buf.extend_from_slice(&ADXM_LOCK_OP_POP_INC_MEM);
        buf.extend_from_slice(&lock_depth_va.to_le_bytes());
        buf.push(0xC3);
        assert_eq!(buf.len(), ADXM_LOCK_LEN);
        buf
    }

    fn build_synthetic_adxm_unlock_bytes(
        lock_depth_va: u32,
        iat_get_cur: u32,
        iat_set_prio: u32,
        spin_handle_va: u32,
        prev_prio_va: u32,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(ADXM_UNLOCK_LEN);
        buf.push(0xA1);
        buf.extend_from_slice(&lock_depth_va.to_le_bytes());
        buf.extend_from_slice(&ADXM_UNLOCK_OP_DEC_MOV);
        buf.extend_from_slice(&lock_depth_va.to_le_bytes());
        buf.extend_from_slice(&ADXM_UNLOCK_OP_JNZ_PUSH_CALL);
        buf.extend_from_slice(&iat_get_cur.to_le_bytes());
        buf.extend_from_slice(&ADXM_UNLOCK_OP_MOV_ESI_MOV_EAX);
        buf.extend_from_slice(&spin_handle_va.to_le_bytes());
        buf.extend_from_slice(&ADXM_UNLOCK_OP_PUSH_EAX_CALL);
        buf.extend_from_slice(&0x0060_0018u32.to_le_bytes());
        buf.extend_from_slice(&ADXM_UNLOCK_OP_MOV_ECX);
        buf.extend_from_slice(&prev_prio_va.to_le_bytes());
        buf.extend_from_slice(&ADXM_UNLOCK_OP_PUSH_ECX_ESI_CALL);
        buf.extend_from_slice(&iat_set_prio.to_le_bytes());
        buf.extend_from_slice(&ADXM_UNLOCK_OP_EPILOGUE);
        assert_eq!(buf.len(), ADXM_UNLOCK_LEN);
        buf
    }

    #[test]
    fn detect_adxm_lock_in_slice_matches_valid_and_rejects_mismatched_operands() {
        let mut bytes = vec![0x90u8; 16];
        let lock_offset = bytes.len();
        bytes.extend(build_synthetic_adxm_lock_bytes(
            0x0080_0100,
            0x0060_0000,
            0x0060_0004,
            0x0080_0200,
            0x0080_0300,
        ));
        bytes.extend_from_slice(&[0xCC; 12]);
        let unlock_offset = bytes.len();
        bytes.extend(build_synthetic_adxm_unlock_bytes(
            0x0080_0100,
            0x0060_0000,
            0x0060_0004,
            0x0080_0200,
            0x0080_0300,
        ));

        let base_va = 0x0055_f980;
        let matched = detect_adxm_lock_in_slice(base_va, &bytes)
            .expect("synthetic ADXM_Lock/Unlock pair should match");
        assert_eq!(matched.lock_va, base_va + lock_offset as u32);
        assert_eq!(matched.unlock_va, base_va + unlock_offset as u32);
        assert_eq!(matched.lock_depth_va, 0x0080_0100);

        // Corrupting unlock's spin_handle_va operand must reject the pair.
        bytes[unlock_offset + 0x17] ^= 0x01;
        assert_eq!(detect_adxm_lock_in_slice(base_va, &bytes), None);
    }

    #[test]
    fn adxm_mutex_shim_supports_recursive_locking_mutual_exclusion_and_shutdown() {
        let matched = AdxmLockMatch {
            lock_va: 0x0055_f990,
            unlock_va: 0x0055_f9e0,
            lock_depth_va: 0x04,
        };
        let mut guest_mem = [0u8; 16];
        let arena_base = guest_mem.as_mut_ptr() as usize;
        let shim = Arc::new(AdxmMutexShim::new(matched));
        let shutdown = Arc::new(AtomicBool::new(false));

        assert_eq!(shim.lock(1, arena_base, &shutdown), 1);
        assert_eq!(shim.lock(1, arena_base, &shutdown), 2);
        assert_eq!(u32::from_le_bytes(guest_mem[4..8].try_into().unwrap()), 2);

        let acquired_by_other = Arc::new(AtomicBool::new(false));
        let other_entered = Arc::new(AtomicBool::new(false));
        let shim_t2 = Arc::clone(&shim);
        let shut_t2 = Arc::clone(&shutdown);
        let acq_t2 = Arc::clone(&acquired_by_other);
        let ent_t2 = Arc::clone(&other_entered);
        let t2 = std::thread::spawn(move || {
            ent_t2.store(true, Ordering::Release);
            let depth = shim_t2.lock(2, 0, &shut_t2);
            acq_t2.store(depth > 0, Ordering::Release);
            shim_t2.unlock(2, 0);
        });

        while !other_entered.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(15));
        assert!(!acquired_by_other.load(Ordering::Acquire));

        assert_eq!(shim.unlock(1, arena_base), 1);
        std::thread::sleep(Duration::from_millis(10));
        assert!(!acquired_by_other.load(Ordering::Acquire));

        assert_eq!(shim.unlock(1, arena_base), 0);
        t2.join().unwrap();
        assert!(acquired_by_other.load(Ordering::Acquire));

        // Verify shutdown wakes blocked worker without granting ownership.
        assert_eq!(shim.lock(1, arena_base, &shutdown), 1);
        let shim_t3 = Arc::clone(&shim);
        let shut_t3 = Arc::clone(&shutdown);
        let t3 = std::thread::spawn(move || shim_t3.lock(3, 0, &shut_t3));
        std::thread::sleep(Duration::from_millis(15));
        shutdown.store(true, Ordering::Release);
        shim.notify_all();
        assert_eq!(t3.join().unwrap(), 0);
        shim.release_if_owned(1, arena_base);
        assert_eq!(u32::from_le_bytes(guest_mem[4..8].try_into().unwrap()), 0);
    }
}
