use std::collections::HashMap;
use std::fmt;

use toluca_core::GuestAddr;
use toluca_core::aot::ImportSymbol;

use crate::sha1::sha1;

pub const DOS_MAGIC: u16 = 0x5A4D;
pub const PE_SIGNATURE: [u8; 4] = *b"PE\0\0";
pub const IMAGE_FILE_MACHINE_I386: u16 = 0x014C;
pub const PE32_OPTIONAL_MAGIC: u16 = 0x010B;
pub const IMAGE_FILE_DLL: u16 = 0x2000;

pub const IMAGE_SCN_CNT_CODE: u32 = 0x0000_0020;
pub const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
pub const IMAGE_SCN_MEM_READ: u32 = 0x4000_0000;
pub const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

pub const DIR_EXPORT: usize = 0;
pub const DIR_IMPORT: usize = 1;
pub const DIR_TLS: usize = 9;
pub const DIR_IAT: usize = 12;

pub const ORDINAL_FLAG_32: u32 = 0x8000_0000;

const DOS_HEADER_MIN_SIZE: usize = 64;
const DOS_LFANEW_OFFSET: usize = 0x3C;
const PE_SIGNATURE_SIZE: usize = 4;
const COFF_HEADER_SIZE: usize = 20;
const PE32_OPT_HEADER_FIXED_SIZE: usize = 96;
const DATA_DIR_ENTRY_SIZE: usize = 8;
const MAX_DATA_DIRECTORIES: usize = 16;
const SECTION_HEADER_SIZE: usize = 40;
const IMPORT_DESCRIPTOR_SIZE: usize = 20;
const EXPORT_DIRECTORY_SIZE: usize = 40;
const TLS_CALLBACKS_OFFSET: usize = 12;
const TLS_DIR32_MIN_SIZE: u32 = 16;
const MAX_IMPORT_DESCRIPTORS: usize = 1024;
const MAX_THUNKS_PER_DLL: usize = 16384;
const MAX_TLS_CALLBACKS: usize = 64;
const MAX_EXPORTS: usize = 65536;
const ORDINAL_MASK_16: u32 = 0xFFFF;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeError {
    TooSmall,
    InvalidDosMagic,
    InvalidPeSignature,
    UnsupportedMachine(u16),
    UnsupportedOptionalMagic(u16),
    TruncatedHeader,
    InvalidSectionTable,
    InvalidImportDirectory,
    InvalidRva(u32),
}

impl fmt::Display for PeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall => write!(f, "file is too small to be a valid PE32 binary"),
            Self::InvalidDosMagic => write!(f, "missing MZ DOS header magic"),
            Self::InvalidPeSignature => write!(f, "missing PE\\0\\0 signature"),
            Self::UnsupportedMachine(m) => {
                write!(f, "unsupported COFF machine 0x{m:04x} (expected x86 0x014c)")
            }
            Self::UnsupportedOptionalMagic(m) => {
                write!(f, "unsupported optional header magic 0x{m:04x} (expected PE32 0x010b)")
            }
            Self::TruncatedHeader => write!(f, "truncated PE header"),
            Self::InvalidSectionTable => write!(f, "invalid or truncated PE section table"),
            Self::InvalidImportDirectory => write!(f, "corrupt PE import directory"),
            Self::InvalidRva(rva) => write!(f, "invalid PE RVA 0x{rva:08x}"),
        }
    }
}

impl std::error::Error for PeError {}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DataDirectory {
    pub rva: u32,
    pub size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeSection {
    pub name: String,
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub raw_data_size: u32,
    pub raw_data_offset: u32,
    pub characteristics: u32,
}

impl PeSection {
    pub const fn is_executable(&self) -> bool {
        (self.characteristics & IMAGE_SCN_MEM_EXECUTE) != 0
    }

    pub const fn is_readable(&self) -> bool {
        (self.characteristics & IMAGE_SCN_MEM_READ) != 0
    }

    pub const fn is_writable(&self) -> bool {
        (self.characteristics & IMAGE_SCN_MEM_WRITE) != 0
    }

    pub fn mapped_size(&self) -> u32 {
        self.virtual_size.max(self.raw_data_size)
    }

    pub fn va_range(&self, image_base: GuestAddr) -> (GuestAddr, GuestAddr) {
        let start = image_base.saturating_add(self.virtual_address);
        let end = start.saturating_add(self.mapped_size());
        (start, end)
    }

    pub fn contains_va(&self, va: GuestAddr, image_base: GuestAddr) -> bool {
        let (start, end) = self.va_range(image_base);
        va >= start && va < end
    }

    pub fn raw_bytes<'a>(&self, file_bytes: &'a [u8]) -> &'a [u8] {
        let start = self.raw_data_offset as usize;
        if start >= file_bytes.len() || self.raw_data_size == 0 {
            return &[];
        }
        let end = start.saturating_add(self.raw_data_size as usize).min(file_bytes.len());
        &file_bytes[start..end]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeImport {
    pub iat_va: GuestAddr,
    pub dll_name: String,
    pub symbol: ImportSymbol,
}

#[derive(Clone, Debug)]
pub struct PeImage {
    pub raw: Vec<u8>,
    pub sha1: [u8; 20],
    pub machine: u16,
    pub characteristics: u16,
    pub time_date_stamp: u32,
    pub image_base: GuestAddr,
    pub entry_point_rva: u32,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub size_of_image: u32,
    pub data_directories: Vec<DataDirectory>,
    pub sections: Vec<PeSection>,
    pub imports: Vec<PeImport>,
    pub iat_map: HashMap<GuestAddr, usize>,
    pub tls_callbacks: Vec<GuestAddr>,
    pub exports: Vec<GuestAddr>,
}

impl PeImage {
    pub fn parse(bytes: Vec<u8>) -> Result<Self, PeError> {
        if bytes.len() < DOS_HEADER_MIN_SIZE {
            return Err(PeError::TooSmall);
        }
        if read_u16(&bytes, 0)? != DOS_MAGIC {
            return Err(PeError::InvalidDosMagic);
        }

        let pe_offset = read_u32(&bytes, DOS_LFANEW_OFFSET)? as usize;
        let sig_end = pe_offset
            .checked_add(PE_SIGNATURE_SIZE)
            .ok_or(PeError::TruncatedHeader)?;
        if sig_end > bytes.len() {
            return Err(PeError::TruncatedHeader);
        }
        if bytes[pe_offset..sig_end] != PE_SIGNATURE {
            return Err(PeError::InvalidPeSignature);
        }

        let coff_start = sig_end;
        let coff_end = coff_start
            .checked_add(COFF_HEADER_SIZE)
            .ok_or(PeError::TruncatedHeader)?;
        if coff_end > bytes.len() {
            return Err(PeError::TruncatedHeader);
        }

        let machine = read_u16(&bytes, coff_start)?;
        if machine != IMAGE_FILE_MACHINE_I386 {
            return Err(PeError::UnsupportedMachine(machine));
        }

        let number_of_sections = read_u16(&bytes, coff_start + 2)? as usize;
        let time_date_stamp = read_u32(&bytes, coff_start + 4)?;
        let size_of_optional_header = read_u16(&bytes, coff_start + 16)? as usize;
        let characteristics = read_u16(&bytes, coff_start + 18)?;

        if size_of_optional_header < PE32_OPT_HEADER_FIXED_SIZE {
            return Err(PeError::TruncatedHeader);
        }

        let opt_start = coff_end;
        let opt_end = opt_start
            .checked_add(size_of_optional_header)
            .ok_or(PeError::TruncatedHeader)?;
        if opt_end > bytes.len() {
            return Err(PeError::TruncatedHeader);
        }

        let opt_magic = read_u16(&bytes, opt_start)?;
        if opt_magic != PE32_OPTIONAL_MAGIC {
            return Err(PeError::UnsupportedOptionalMagic(opt_magic));
        }

        let entry_point_rva = read_u32(&bytes, opt_start + 16)?;
        let image_base = read_u32(&bytes, opt_start + 28)?;
        let section_alignment = read_u32(&bytes, opt_start + 32)?;
        let file_alignment = read_u32(&bytes, opt_start + 36)?;
        let size_of_image = read_u32(&bytes, opt_start + 56)?;
        let number_of_rva_and_sizes = read_u32(&bytes, opt_start + 92)? as usize;

        let dir_area_bytes = size_of_optional_header - PE32_OPT_HEADER_FIXED_SIZE;
        let max_dirs_in_opt = dir_area_bytes / DATA_DIR_ENTRY_SIZE;
        let dir_count = number_of_rva_and_sizes
            .min(max_dirs_in_opt)
            .min(MAX_DATA_DIRECTORIES);

        let mut data_directories = Vec::with_capacity(dir_count);
        let dirs_base = opt_start + PE32_OPT_HEADER_FIXED_SIZE;
        for idx in 0..dir_count {
            let off = dirs_base + idx * DATA_DIR_ENTRY_SIZE;
            data_directories.push(DataDirectory {
                rva: read_u32(&bytes, off)?,
                size: read_u32(&bytes, off + 4)?,
            });
        }

        let sec_table_bytes = number_of_sections
            .checked_mul(SECTION_HEADER_SIZE)
            .ok_or(PeError::InvalidSectionTable)?;
        let sec_table_end = opt_end
            .checked_add(sec_table_bytes)
            .ok_or(PeError::InvalidSectionTable)?;
        if sec_table_end > bytes.len() {
            return Err(PeError::InvalidSectionTable);
        }

        let mut sections = Vec::with_capacity(number_of_sections);
        for idx in 0..number_of_sections {
            let off = opt_end + idx * SECTION_HEADER_SIZE;
            let name_bytes = &bytes[off..off + 8];
            let name_len = name_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(name_bytes.len());
            let name = String::from_utf8_lossy(&name_bytes[..name_len]).into_owned();

            sections.push(PeSection {
                name,
                virtual_size: read_u32(&bytes, off + 8)?,
                virtual_address: read_u32(&bytes, off + 12)?,
                raw_data_size: read_u32(&bytes, off + 16)?,
                raw_data_offset: read_u32(&bytes, off + 20)?,
                characteristics: read_u32(&bytes, off + 36)?,
            });
        }

        let digest = sha1(&bytes);
        let mut image = Self {
            raw: bytes,
            sha1: digest,
            machine,
            characteristics,
            time_date_stamp,
            image_base,
            entry_point_rva,
            section_alignment,
            file_alignment,
            size_of_image,
            data_directories,
            sections,
            imports: Vec::new(),
            iat_map: HashMap::new(),
            tls_callbacks: Vec::new(),
            exports: Vec::new(),
        };

        image.parse_imports()?;
        image.parse_tls_callbacks();
        image.parse_exports();
        Ok(image)
    }

    pub const fn entry_va(&self) -> GuestAddr {
        self.image_base.wrapping_add(self.entry_point_rva)
    }

    pub const fn is_dll(&self) -> bool {
        (self.characteristics & IMAGE_FILE_DLL) != 0
    }

    pub fn rva_to_file_offset(&self, rva: u32) -> Option<usize> {
        for sec in &self.sections {
            let end_rva = sec.virtual_address.saturating_add(sec.mapped_size());
            if rva >= sec.virtual_address && rva < end_rva {
                let delta = rva - sec.virtual_address;
                if delta >= sec.raw_data_size {
                    return None;
                }
                let file_off = sec.raw_data_offset.checked_add(delta)? as usize;
                return (file_off < self.raw.len()).then_some(file_off);
            }
        }
        None
    }

    pub fn va_to_file_offset(&self, va: GuestAddr) -> Option<usize> {
        let rva = va.checked_sub(self.image_base)?;
        self.rva_to_file_offset(rva)
    }

    pub fn read_bytes_at_va(&self, va: GuestAddr, len: usize) -> Option<&[u8]> {
        let rva = va.checked_sub(self.image_base)?;
        self.read_bytes_at_rva(rva, len)
    }

    pub fn read_u32_at_va(&self, va: GuestAddr) -> Option<u32> {
        let bytes = self.read_bytes_at_va(va, 4)?;
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn section_for_va(&self, va: GuestAddr) -> Option<&PeSection> {
        self.sections
            .iter()
            .find(|sec| sec.contains_va(va, self.image_base))
    }

    pub fn section_by_name(&self, name: &str) -> Option<&PeSection> {
        self.sections.iter().find(|sec| sec.name == name)
    }

    pub fn import_at_va(&self, va: GuestAddr) -> Option<&PeImport> {
        self.iat_map.get(&va).map(|&idx| &self.imports[idx])
    }

    pub fn executable_sections(&self) -> impl Iterator<Item = &PeSection> {
        self.sections.iter().filter(|sec| sec.is_executable())
    }

    pub fn data_sections(&self) -> impl Iterator<Item = &PeSection> {
        self.sections
            .iter()
            .filter(|sec| sec.is_readable() && !sec.is_executable())
    }

    fn data_dir(&self, index: usize) -> Option<DataDirectory> {
        self.data_directories
            .get(index)
            .copied()
            .filter(|dir| dir.rva != 0 && dir.size != 0)
    }

    fn read_bytes_at_rva(&self, rva: u32, len: usize) -> Option<&[u8]> {
        for sec in &self.sections {
            let end_rva = sec.virtual_address.saturating_add(sec.mapped_size());
            if rva >= sec.virtual_address && rva < end_rva {
                let delta = (rva - sec.virtual_address) as usize;
                let raw = sec.raw_bytes(&self.raw);
                let end = delta.checked_add(len)?;
                if end <= raw.len() {
                    return Some(&raw[delta..end]);
                }
                return None;
            }
        }
        None
    }

    fn read_u32_at_rva(&self, rva: u32) -> Option<u32> {
        let bytes = self.read_bytes_at_rva(rva, 4)?;
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_cstr_at_rva(&self, rva: u32) -> Option<String> {
        for sec in &self.sections {
            let end_rva = sec.virtual_address.saturating_add(sec.mapped_size());
            if rva >= sec.virtual_address && rva < end_rva {
                let delta = (rva - sec.virtual_address) as usize;
                let raw = sec.raw_bytes(&self.raw);
                if delta >= raw.len() {
                    return None;
                }
                let tail = &raw[delta..];
                let nul_pos = tail.iter().position(|&b| b == 0)?;
                return Some(String::from_utf8_lossy(&tail[..nul_pos]).into_owned());
            }
        }
        None
    }

    fn parse_imports(&mut self) -> Result<(), PeError> {
        let Some(import_dir) = self.data_dir(DIR_IMPORT) else {
            return Ok(());
        };

        for desc_idx in 0..MAX_IMPORT_DESCRIPTORS {
            let desc_rva = import_dir
                .rva
                .checked_add((desc_idx * IMPORT_DESCRIPTOR_SIZE) as u32)
                .ok_or(PeError::InvalidImportDirectory)?;
            let desc_bytes = self
                .read_bytes_at_rva(desc_rva, IMPORT_DESCRIPTOR_SIZE)
                .ok_or(PeError::InvalidImportDirectory)?;

            let original_first_thunk = read_u32(desc_bytes, 0)?;
            let name_rva = read_u32(desc_bytes, 12)?;
            let first_thunk = read_u32(desc_bytes, 16)?;

            if original_first_thunk == 0 && name_rva == 0 && first_thunk == 0 {
                break;
            }
            if name_rva == 0 || first_thunk == 0 {
                return Err(PeError::InvalidImportDirectory);
            }

            let dll_name = self
                .read_cstr_at_rva(name_rva)
                .ok_or(PeError::InvalidImportDirectory)?;
            let lookup_rva = if original_first_thunk != 0 {
                original_first_thunk
            } else {
                first_thunk
            };

            for thunk_idx in 0..MAX_THUNKS_PER_DLL {
                let entry_off = (thunk_idx * 4) as u32;
                let thunk_rva = lookup_rva
                    .checked_add(entry_off)
                    .ok_or(PeError::InvalidImportDirectory)?;
                let thunk = self
                    .read_u32_at_rva(thunk_rva)
                    .ok_or(PeError::InvalidImportDirectory)?;
                if thunk == 0 {
                    break;
                }

                let iat_rva = first_thunk
                    .checked_add(entry_off)
                    .ok_or(PeError::InvalidImportDirectory)?;
                let iat_va = self.image_base.wrapping_add(iat_rva);

                let symbol = if (thunk & ORDINAL_FLAG_32) != 0 {
                    ImportSymbol::Ordinal((thunk & ORDINAL_MASK_16) as u16)
                } else {
                    let name_offset_rva = thunk
                        .checked_add(2)
                        .ok_or(PeError::InvalidImportDirectory)?;
                    let sym_name = self
                        .read_cstr_at_rva(name_offset_rva)
                        .ok_or(PeError::InvalidImportDirectory)?;
                    ImportSymbol::Name(sym_name)
                };

                let import_idx = self.imports.len();
                self.imports.push(PeImport {
                    iat_va,
                    dll_name: dll_name.clone(),
                    symbol,
                });
                self.iat_map.insert(iat_va, import_idx);
            }
        }

        Ok(())
    }

    fn parse_tls_callbacks(&mut self) {
        let Some(tls_dir) = self.data_dir(DIR_TLS) else {
            return;
        };
        if tls_dir.size < TLS_DIR32_MIN_SIZE {
            return;
        }
        let Some(callbacks_va) = self.read_u32_at_rva(tls_dir.rva + TLS_CALLBACKS_OFFSET as u32)
        else {
            return;
        };
        if callbacks_va == 0 {
            return;
        }

        for idx in 0..MAX_TLS_CALLBACKS {
            let slot_va = callbacks_va.wrapping_add((idx * 4) as u32);
            let Some(cb_va) = self.read_u32_at_va(slot_va) else {
                break;
            };
            if cb_va == 0 {
                break;
            }
            if self
                .section_for_va(cb_va)
                .is_some_and(PeSection::is_executable)
            {
                self.tls_callbacks.push(cb_va);
            }
        }
    }

    fn parse_exports(&mut self) {
        let Some(export_dir) = self.data_dir(DIR_EXPORT) else {
            return;
        };
        if export_dir.size < EXPORT_DIRECTORY_SIZE as u32 {
            return;
        }
        let Some(dir_bytes) = self.read_bytes_at_rva(export_dir.rva, EXPORT_DIRECTORY_SIZE) else {
            return;
        };
        let Ok(num_functions) = read_u32(dir_bytes, 20) else {
            return;
        };
        let Ok(addr_of_functions_rva) = read_u32(dir_bytes, 28) else {
            return;
        };

        let export_end = export_dir.rva.saturating_add(export_dir.size);
        let count = (num_functions as usize).min(MAX_EXPORTS);
        for idx in 0..count {
            let Some(func_rva) = self.read_u32_at_rva(addr_of_functions_rva + (idx * 4) as u32)
            else {
                break;
            };
            if func_rva == 0 || (func_rva >= export_dir.rva && func_rva < export_end) {
                continue;
            }
            let func_va = self.image_base.wrapping_add(func_rva);
            if self
                .section_for_va(func_va)
                .is_some_and(PeSection::is_executable)
            {
                self.exports.push(func_va);
            }
        }
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, PeError> {
    let end = offset.checked_add(2).ok_or(PeError::TruncatedHeader)?;
    let slice = bytes.get(offset..end).ok_or(PeError::TruncatedHeader)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PeError> {
    let end = offset.checked_add(4).ok_or(PeError::TruncatedHeader)?;
    let slice = bytes.get(offset..end).ok_or(PeError::TruncatedHeader)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    pub struct TestSectionSpec<'a> {
        pub name: &'a str,
        pub virtual_address: u32,
        pub characteristics: u32,
        pub data: &'a [u8],
    }

    pub fn build_test_pe32(
        image_base: u32,
        entry_point_rva: u32,
        characteristics: u16,
        data_dirs: &[(usize, DataDirectory)],
        sections: &[TestSectionSpec<'_>],
    ) -> Vec<u8> {
        let pe_offset = 0x80usize;
        let opt_size = PE32_OPT_HEADER_FIXED_SIZE + MAX_DATA_DIRECTORIES * DATA_DIR_ENTRY_SIZE;
        let sec_table_offset = pe_offset + PE_SIGNATURE_SIZE + COFF_HEADER_SIZE + opt_size;
        let raw_headers_end = sec_table_offset + sections.len() * SECTION_HEADER_SIZE;
        let file_alignment = 0x200usize;
        let section_alignment = 0x1000u32;
        let size_of_headers = raw_headers_end.div_ceil(file_alignment) * file_alignment;

        let mut total_file_size = size_of_headers;
        let mut raw_offsets = Vec::with_capacity(sections.len());
        let mut raw_sizes = Vec::with_capacity(sections.len());
        for sec in sections {
            raw_offsets.push(total_file_size);
            let aligned = sec.data.len().max(1).div_ceil(file_alignment) * file_alignment;
            raw_sizes.push(aligned);
            total_file_size += aligned;
        }

        let mut buf = vec![0u8; total_file_size];
        buf[0..2].copy_from_slice(&DOS_MAGIC.to_le_bytes());
        buf[DOS_LFANEW_OFFSET..DOS_LFANEW_OFFSET + 4]
            .copy_from_slice(&(pe_offset as u32).to_le_bytes());

        buf[pe_offset..pe_offset + 4].copy_from_slice(&PE_SIGNATURE);
        let coff = pe_offset + 4;
        buf[coff..coff + 2].copy_from_slice(&IMAGE_FILE_MACHINE_I386.to_le_bytes());
        buf[coff + 2..coff + 4].copy_from_slice(&(sections.len() as u16).to_le_bytes());
        buf[coff + 16..coff + 18].copy_from_slice(&(opt_size as u16).to_le_bytes());
        buf[coff + 18..coff + 20].copy_from_slice(&characteristics.to_le_bytes());

        let opt = coff + COFF_HEADER_SIZE;
        buf[opt..opt + 2].copy_from_slice(&PE32_OPTIONAL_MAGIC.to_le_bytes());
        buf[opt + 16..opt + 20].copy_from_slice(&entry_point_rva.to_le_bytes());
        buf[opt + 28..opt + 32].copy_from_slice(&image_base.to_le_bytes());
        buf[opt + 32..opt + 36].copy_from_slice(&section_alignment.to_le_bytes());
        buf[opt + 36..opt + 40].copy_from_slice(&(file_alignment as u32).to_le_bytes());
        let last_end = sections
            .last()
            .map(|s| s.virtual_address + (s.data.len() as u32).max(section_alignment))
            .unwrap_or(section_alignment);
        let size_of_image = last_end.div_ceil(section_alignment) * section_alignment;
        buf[opt + 56..opt + 60].copy_from_slice(&size_of_image.to_le_bytes());
        buf[opt + 60..opt + 64].copy_from_slice(&(size_of_headers as u32).to_le_bytes());
        buf[opt + 92..opt + 96].copy_from_slice(&(MAX_DATA_DIRECTORIES as u32).to_le_bytes());

        let dirs_base = opt + PE32_OPT_HEADER_FIXED_SIZE;
        for &(idx, dir) in data_dirs {
            let off = dirs_base + idx * DATA_DIR_ENTRY_SIZE;
            buf[off..off + 4].copy_from_slice(&dir.rva.to_le_bytes());
            buf[off + 4..off + 8].copy_from_slice(&dir.size.to_le_bytes());
        }

        for (idx, sec) in sections.iter().enumerate() {
            let off = sec_table_offset + idx * SECTION_HEADER_SIZE;
            let name_bytes = sec.name.as_bytes();
            let copy_len = name_bytes.len().min(8);
            buf[off..off + copy_len].copy_from_slice(&name_bytes[..copy_len]);
            buf[off + 8..off + 12].copy_from_slice(&(sec.data.len() as u32).to_le_bytes());
            buf[off + 12..off + 16].copy_from_slice(&sec.virtual_address.to_le_bytes());
            buf[off + 16..off + 20].copy_from_slice(&(raw_sizes[idx] as u32).to_le_bytes());
            buf[off + 20..off + 24].copy_from_slice(&(raw_offsets[idx] as u32).to_le_bytes());
            buf[off + 36..off + 40].copy_from_slice(&sec.characteristics.to_le_bytes());

            let raw_start = raw_offsets[idx];
            buf[raw_start..raw_start + sec.data.len()].copy_from_slice(sec.data);
        }

        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_helpers::{TestSectionSpec, build_test_pe32};

    #[test]
    fn rejects_invalid_or_non_pe32_inputs() {
        assert_eq!(PeImage::parse(vec![0; 16]).unwrap_err(), PeError::TooSmall);
        assert_eq!(
            PeImage::parse(vec![0; 128]).unwrap_err(),
            PeError::InvalidDosMagic
        );

        let mut bad_sig = build_test_pe32(0x0040_0000, 0x1000, 0x0102, &[], &[]);
        bad_sig[0x80] = b'X';
        assert_eq!(
            PeImage::parse(bad_sig).unwrap_err(),
            PeError::InvalidPeSignature
        );

        let mut bad_mach = build_test_pe32(0x0040_0000, 0x1000, 0x0102, &[], &[]);
        bad_mach[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        assert_eq!(
            PeImage::parse(bad_mach).unwrap_err(),
            PeError::UnsupportedMachine(0x8664)
        );
    }

    #[test]
    fn parses_sections_and_imports_with_original_and_fallback_thunks() {
        // Build .rdata at RVA 0x2000 containing import descriptors, thunks, strings
        let rdata_rva = 0x2000u32;
        let mut rdata = vec![0u8; 256];

        // Descriptor 0 (OriginalFirstThunk = 0x2060, Name = 0x2080, FirstThunk = 0x2070)
        rdata[0..4].copy_from_slice(&0x2060u32.to_le_bytes());
        rdata[12..16].copy_from_slice(&0x2080u32.to_le_bytes());
        rdata[16..20].copy_from_slice(&0x2070u32.to_le_bytes());
        // Descriptor 1 (OriginalFirstThunk = 0 fallback, Name = 0x20A0, FirstThunk = 0x2078)
        rdata[20..24].copy_from_slice(&0u32.to_le_bytes());
        rdata[32..36].copy_from_slice(&0x20A0u32.to_le_bytes());
        rdata[36..40].copy_from_slice(&0x2078u32.to_le_bytes());

        // OriginalFirstThunk for desc 0 -> Hint/Name at 0x20B0
        rdata[0x60..0x64].copy_from_slice(&0x20B0u32.to_le_bytes());
        // FirstThunk for desc 1 -> Ordinal 42
        rdata[0x78..0x7C].copy_from_slice(&(ORDINAL_FLAG_32 | 42).to_le_bytes());

        rdata[0x80..0x8D].copy_from_slice(b"KERNEL32.dll\0");
        rdata[0xA0..0xAB].copy_from_slice(b"USER32.dll\0");
        rdata[0xB2..0xBE].copy_from_slice(b"ExitProcess\0");

        let bytes = build_test_pe32(
            0x0040_0000,
            0x1000,
            0x0102,
            &[(DIR_IMPORT, DataDirectory { rva: rdata_rva, size: 60 })],
            &[
                TestSectionSpec {
                    name: ".text",
                    virtual_address: 0x1000,
                    characteristics: IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ,
                    data: &[0xC3],
                },
                TestSectionSpec {
                    name: ".rdata",
                    virtual_address: rdata_rva,
                    characteristics: IMAGE_SCN_MEM_READ,
                    data: &rdata,
                },
            ],
        );

        let pe = PeImage::parse(bytes).unwrap();
        assert_eq!(pe.entry_va(), 0x0040_1000);
        assert!(!pe.is_dll());
        assert_eq!(pe.imports.len(), 2);

        let imp0 = pe.import_at_va(0x0040_2070).unwrap();
        assert_eq!(imp0.dll_name, "KERNEL32.dll");
        assert_eq!(imp0.symbol, ImportSymbol::Name("ExitProcess".to_owned()));

        let imp1 = pe.import_at_va(0x0040_2078).unwrap();
        assert_eq!(imp1.dll_name, "USER32.dll");
        assert_eq!(imp1.symbol, ImportSymbol::Ordinal(42));
    }
}
