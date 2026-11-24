use std::fmt;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::ptr::NonNull;

use toluca_core::GuestAddr;
use toluca_core::aot::{AOT_CODE_ALIGNMENT, AotError, AotManifest, TargetArch};
use toluca_core::bridge::RecompiledFn;
use toluca_core::pe::PeImage;

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

        let code_base = NonNull::new(ptr.cast::<u8>());
        Ok(Self {
            manifest,
            code_base,
            code_len,
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

    pub fn resolve_va(&self, va: GuestAddr) -> Option<RecompiledFn> {
        let entry = self.manifest.lookup_va(va)?;
        let base = self.code_base?;
        let offset = entry.code_offset as usize;
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

fn query_host_page_size() -> usize {
    let raw = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(raw).unwrap_or(4096).max(4096)
}
