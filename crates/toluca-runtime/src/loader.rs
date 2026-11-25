use std::fmt;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::ptr::NonNull;

use toluca_core::GuestAddr;
pub use toluca_core::aot::flush_icache;
use toluca_core::aot::{AOT_CODE_ALIGNMENT, AotError, AotManifest, DispatchEntry, TargetArch};
use toluca_core::bridge::RecompiledFn;
use toluca_core::pe::PeImage;

const VA_HASH_MULT: u32 = 0x9E37_79B1;
const VA_TABLE_MIN_LEN: usize = 64;

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct VaCacheEntry {
    va: u32,
    code_offset: u32,
}

#[derive(Debug)]
pub enum LoaderError {
    Io(std::io::Error),
    Aot(AotError),
    UnsupportedHostArch,
    ArchMismatch { expected: TargetArch, actual: u16 },
    MisalignedCodeSection { file_offset: u64, host_page_size: usize },
    CodeTooLarge(u64),
    MmapFailed(i32),
    SourceSha1Mismatch { expected: [u8; 20], actual: [u8; 20] },
}

impl fmt::Display for LoaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "failed to open AOT image: {err}"),
            Self::Aot(err) => write!(f, "invalid AOT image: {err}"),
            Self::UnsupportedHostArch => write!(f, "unsupported host CPU architecture"),
            Self::ArchMismatch { expected, actual } => {
                write!(f, "AOT arch mismatch: expected {expected:?}, got {actual}")
            }
            Self::MisalignedCodeSection {
                file_offset,
                host_page_size,
            } => write!(
                f,
                "AOT code section offset 0x{file_offset:x} is not aligned to {host_page_size} bytes"
            ),
            Self::CodeTooLarge(len) => write!(f, "AOT code section too large ({len} bytes)"),
            Self::MmapFailed(errno) => write!(f, "mmap(PROT_READ|PROT_EXEC) failed with errno {errno}"),
            Self::SourceSha1Mismatch { .. } => {
                write!(f, "AOT source SHA-1 does not match target PE32 binary")
            }
        }
    }
}

impl std::error::Error for LoaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Aot(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LoaderError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<AotError> for LoaderError {
    fn from(err: AotError) -> Self {
        Self::Aot(err)
    }
}

pub struct AotCodeImage {
    manifest: AotManifest,
    code_base: Option<NonNull<u8>>,
    code_len: usize,
    va_table: Vec<VaCacheEntry>,
    va_mask: usize,
    va_shift: u32,
}

unsafe impl Send for AotCodeImage {}
unsafe impl Sync for AotCodeImage {}

impl AotCodeImage {
    pub fn load(path: &Path) -> Result<Self, LoaderError> {
        let mut file = File::open(path)?;
        let manifest = AotManifest::read_from(&mut file, true)?;

        let expected = TargetArch::current().ok_or(LoaderError::UnsupportedHostArch)?;
        if TargetArch::from_u16(manifest.header.arch) != Some(expected) {
            return Err(LoaderError::ArchMismatch {
                expected,
                actual: manifest.header.arch,
            });
        }

        let (va_table, va_mask, va_shift) = build_va_table(&manifest.dispatch);

        let code_sec = manifest.code_section;
        let host_page_size = query_host_page_size();
        let required_align = AOT_CODE_ALIGNMENT.max(host_page_size as u64);
        if !code_sec.file_offset.is_multiple_of(required_align) {
            return Err(LoaderError::MisalignedCodeSection {
                file_offset: code_sec.file_offset,
                host_page_size,
            });
        }

        if code_sec.byte_len == 0 {
            return Ok(Self {
                manifest,
                code_base: None,
                code_len: 0,
                va_table,
                va_mask,
                va_shift,
            });
        }

        let code_len =
            usize::try_from(code_sec.byte_len).map_err(|_| LoaderError::CodeTooLarge(code_sec.byte_len))?;
        let offset = libc::off_t::try_from(code_sec.file_offset)
            .map_err(|_| LoaderError::CodeTooLarge(code_sec.file_offset))?;

        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                code_len,
                libc::PROT_READ | libc::PROT_EXEC,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                offset,
            )
        };
        if ptr == libc::MAP_FAILED {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return Err(LoaderError::MmapFailed(errno));
        }

        let code_ptr = ptr.cast::<u8>();
        unsafe {
            flush_icache(code_ptr, code_len);
        }

        let code_base = NonNull::new(code_ptr);
        Ok(Self {
            manifest,
            code_base,
            code_len,
            va_table,
            va_mask,
            va_shift,
        })
    }

    pub fn verify_source_pe(&self, pe: &PeImage) -> Result<(), LoaderError> {
        if self.manifest.header.source_sha1 != pe.sha1 {
            return Err(LoaderError::SourceSha1Mismatch {
                expected: self.manifest.header.source_sha1,
                actual: pe.sha1,
            });
        }
        Ok(())
    }

    fn lookup_code_offset(&self, va: GuestAddr) -> Option<u32> {
        if va == 0 {
            return self.manifest.lookup_va(va).map(|entry| entry.code_offset);
        }
        let mask = self.va_mask;
        let mut slot = ((va.wrapping_mul(VA_HASH_MULT)) >> self.va_shift) as usize & mask;
        loop {
            let entry = self.va_table[slot];
            if entry.va == va {
                return Some(entry.code_offset);
            }
            if entry.va == 0 {
                return None;
            }
            slot = (slot + 1) & mask;
        }
    }

    pub fn resolve_va(&self, va: GuestAddr) -> Option<RecompiledFn> {
        let base = self.code_base?;
        let offset = self.lookup_code_offset(va)? as usize;
        if offset >= self.code_len {
            return None;
        }
        unsafe {
            let ptr = base.as_ptr().add(offset);
            Some(std::mem::transmute::<*const u8, RecompiledFn>(ptr))
        }
    }

    pub fn manifest(&self) -> &AotManifest {
        &self.manifest
    }

    pub fn entry_point_va(&self) -> GuestAddr {
        self.manifest.header.entry_guest_va
    }

    pub fn code_range(&self) -> (usize, usize) {
        let base = self.code_base.map_or(0, |p| p.as_ptr() as usize);
        (base, base.saturating_add(self.code_len))
    }

    #[cfg(test)]
    pub(crate) fn empty_for_test() -> Self {
        let manifest = empty_test_manifest(Vec::new());
        let (va_table, va_mask, va_shift) = build_va_table(&manifest.dispatch);
        Self {
            manifest,
            code_base: None,
            code_len: 0,
            va_table,
            va_mask,
            va_shift,
        }
    }
}

impl Drop for AotCodeImage {
    fn drop(&mut self) {
        if let Some(base) = self.code_base {
            unsafe {
                libc::munmap(base.as_ptr().cast(), self.code_len);
            }
        }
    }
}

fn build_va_table(dispatch: &[DispatchEntry]) -> (Vec<VaCacheEntry>, usize, u32) {
    let min_slots = dispatch.len().saturating_mul(2).max(VA_TABLE_MIN_LEN);
    let cap = min_slots.next_power_of_two();
    let mask = cap - 1;
    let shift = 32u32.saturating_sub(cap.trailing_zeros());
    let mut table = vec![VaCacheEntry::default(); cap];

    for entry in dispatch {
        if entry.guest_va == 0 {
            continue;
        }
        let mut slot = ((entry.guest_va.wrapping_mul(VA_HASH_MULT)) >> shift) as usize & mask;
        while table[slot].va != 0 {
            if table[slot].va == entry.guest_va {
                break;
            }
            slot = (slot + 1) & mask;
        }
        table[slot] = VaCacheEntry {
            va: entry.guest_va,
            code_offset: entry.code_offset,
        };
    }

    (table, mask, shift)
}

#[cfg(test)]
fn empty_test_manifest(dispatch: Vec<DispatchEntry>) -> AotManifest {
    use toluca_core::aot::{AotHeader, SectionEntry};
    AotManifest {
        header: AotHeader {
            magic: *b"TLCA",
            format_version: 1,
            arch: 1,
            flags: 0,
            section_count: 0,
            source_sha1: [0; 20],
            entry_guest_va: 0,
            image_base: 0,
            payload_crc32: 0,
            total_file_size: 0,
            reserved: [0; 8],
        },
        code_section: SectionEntry {
            kind: 1,
            flags: 0,
            file_offset: 0,
            byte_len: 0,
            entry_count: 0,
            crc32: 0,
        },
        dispatch,
        relocations: Vec::new(),
        imports: Vec::new(),
    }
}

fn query_host_page_size() -> usize {
    let raw = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(raw).unwrap_or(4096).max(4096)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image_with_dispatch(mut dispatch: Vec<DispatchEntry>, code_len: usize) -> AotCodeImage {
        dispatch.sort_by_key(|entry| entry.guest_va);
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                code_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(ptr, libc::MAP_FAILED, "anonymous mmap for test image failed");
        let code_base = NonNull::new(ptr.cast::<u8>());
        let (va_table, va_mask, va_shift) = build_va_table(&dispatch);
        AotCodeImage {
            manifest: empty_test_manifest(dispatch),
            code_base,
            code_len,
            va_table,
            va_mask,
            va_shift,
        }
    }

    fn fn_offset(img: &AotCodeImage, f: RecompiledFn) -> usize {
        let base = img.code_base.unwrap().as_ptr() as usize;
        (f as usize) - base
    }

    #[test]
    fn resolve_va_handles_hits_misses_collisions_and_zero() {
        // Choose two VAs that deliberately hash to the exact same initial slot when cap == 64.
        let shift = 32 - VA_TABLE_MIN_LEN.trailing_zeros();
        let mask = VA_TABLE_MIN_LEN - 1;
        let va1 = 0x0040_1000u32;
        let bucket1 = ((va1.wrapping_mul(VA_HASH_MULT)) >> shift) as usize & mask;
        let mut va2 = 0x0040_2000u32;
        while (((va2.wrapping_mul(VA_HASH_MULT)) >> shift) as usize & mask) != bucket1 {
            va2 += 0x10;
        }

        let entries = vec![
            DispatchEntry {
                guest_va: 0,
                code_offset: 0x0100,
                code_size: 16,
                flags: 0,
            },
            DispatchEntry {
                guest_va: va1,
                code_offset: 0x0200,
                code_size: 16,
                flags: 0,
            },
            DispatchEntry {
                guest_va: va2,
                code_offset: 0x0300,
                code_size: 16,
                flags: 0,
            },
        ];
        let img = test_image_with_dispatch(entries, 4096);

        assert_eq!(img.resolve_va(va1).map(|f| fn_offset(&img, f)), Some(0x0200));
        assert_eq!(img.resolve_va(va2).map(|f| fn_offset(&img, f)), Some(0x0300));
        assert_eq!(img.resolve_va(0).map(|f| fn_offset(&img, f)), Some(0x0100));
        assert!(img.resolve_va(0x0040_9999).is_none());

        // Missing VA that hashes to the same colliding bucket must still terminate and return None.
        let mut va_miss_same_bucket = va2 + 0x10;
        while (((va_miss_same_bucket.wrapping_mul(VA_HASH_MULT)) >> shift) as usize & mask) != bucket1
            || va_miss_same_bucket == va1
            || va_miss_same_bucket == va2
        {
            va_miss_same_bucket += 0x10;
        }
        assert!(img.resolve_va(va_miss_same_bucket).is_none());

        // Verify empty_for_test safely misses on both zero and non-zero VAs.
        let empty = AotCodeImage::empty_for_test();
        assert!(empty.resolve_va(0).is_none());
        assert!(empty.resolve_va(va1).is_none());
    }
}
