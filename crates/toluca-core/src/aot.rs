use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};

pub const AOT_MAGIC: [u8; 4] = *b"TLCA";
pub const AOT_FORMAT_VERSION: u32 = 1;

pub const AOT_HEADER_SIZE: usize = 64;
pub const AOT_SECTION_ENTRY_SIZE: usize = 32;
pub const AOT_DISPATCH_ENTRY_SIZE: usize = 16;
pub const AOT_RELOC_ENTRY_SIZE: usize = 16;
pub const AOT_IMPORT_ENTRY_SIZE: usize = 16;

pub const AOT_CODE_ALIGNMENT: u64 = 0x10000;
pub const AOT_FUNC_ALIGNMENT: usize = 16;

pub const IMPORT_BY_ORDINAL_FLAG: u16 = 1;
pub const IMPORT_NO_NAME_OFFSET: u32 = u32::MAX;

pub const FUNC_FLAG_ENTRY_POINT: u32 = 1 << 0;
pub const FUNC_FLAG_JUMP_TABLE_TARGET: u32 = 1 << 1;
pub const FUNC_FLAG_THUNK: u32 = 1 << 2;

const CRC32_POLYNOMIAL: u32 = 0xEDB8_8320;
const CRC_STREAM_CHUNK_SIZE: usize = 64 * 1024;
const HEADER_PAD_CHUNK_SIZE: usize = 4096;
const X86_INT3_PADDING: u8 = 0xCC;
const AARCH64_ZERO_PADDING: u8 = 0x00;
const EXPECTED_SECTION_COUNT: usize = 5;
const AARCH64_CALL26_IMM_MASK: u32 = 0x03FF_FFFF;
const AARCH64_CALL26_OP_MASK: u32 = 0xFC00_0000;
const AARCH64_CALL26_RANGE: i64 = 1i64 << 27;

const CRC32_TABLE: [u32; 256] = make_crc32_table();

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut idx = 0usize;
    while idx < 256 {
        let mut crc = idx as u32;
        let mut bit = 0;
        while bit < 8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ CRC32_POLYNOMIAL;
            } else {
                crc >>= 1;
            }
            bit += 1;
        }
        table[idx] = crc;
        idx += 1;
    }
    table
}

pub fn crc32_update(state: u32, data: &[u8]) -> u32 {
    let mut crc = !state;
    for &byte in data {
        let index = ((crc ^ (byte as u32)) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[index];
    }
    !crc
}

pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

#[repr(u16)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TargetArch {
    X86_64 = 1,
    AArch64 = 2,
}

impl TargetArch {
    pub const fn current() -> Option<Self> {
        #[cfg(target_arch = "x86_64")]
        {
            Some(Self::X86_64)
        }
        #[cfg(target_arch = "aarch64")]
        {
            Some(Self::AArch64)
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            None
        }
    }

    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::X86_64),
            2 => Some(Self::AArch64),
            _ => None,
        }
    }

    const fn padding_byte(self) -> u8 {
        match self {
            Self::X86_64 => X86_INT3_PADDING,
            Self::AArch64 => AARCH64_ZERO_PADDING,
        }
    }
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SectionKind {
    Code = 1,
    Dispatch = 2,
    Relocations = 3,
    Imports = 4,
    Strings = 5,
}

impl SectionKind {
    pub const fn from_u32(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Code),
            2 => Some(Self::Dispatch),
            3 => Some(Self::Relocations),
            4 => Some(Self::Imports),
            5 => Some(Self::Strings),
            _ => None,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Dispatch => "dispatch",
            Self::Relocations => "relocations",
            Self::Imports => "imports",
            Self::Strings => "strings",
        }
    }
}

#[repr(u16)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RelocKind {
    X86_64Rel32 = 1,
    AArch64Call26 = 2,
    GotSlot32 = 3,
}

impl RelocKind {
    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::X86_64Rel32),
            2 => Some(Self::AArch64Call26),
            3 => Some(Self::GotSlot32),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AotHeader {
    pub magic: [u8; 4],
    pub format_version: u32,
    pub arch: u16,
    pub flags: u16,
    pub section_count: u32,
    pub source_sha1: [u8; 20],
    pub entry_guest_va: u32,
    pub image_base: u32,
    pub payload_crc32: u32,
    pub total_file_size: u64,
    pub reserved: [u8; 8],
}

impl AotHeader {
    pub fn to_bytes(&self) -> [u8; AOT_HEADER_SIZE] {
        let mut buf = [0u8; AOT_HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4..8].copy_from_slice(&self.format_version.to_le_bytes());
        buf[8..10].copy_from_slice(&self.arch.to_le_bytes());
        buf[10..12].copy_from_slice(&self.flags.to_le_bytes());
        buf[12..16].copy_from_slice(&self.section_count.to_le_bytes());
        buf[16..36].copy_from_slice(&self.source_sha1);
        buf[36..40].copy_from_slice(&self.entry_guest_va.to_le_bytes());
        buf[40..44].copy_from_slice(&self.image_base.to_le_bytes());
        buf[44..48].copy_from_slice(&self.payload_crc32.to_le_bytes());
        buf[48..56].copy_from_slice(&self.total_file_size.to_le_bytes());
        buf[56..64].copy_from_slice(&self.reserved);
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AotError> {
        if bytes.len() != AOT_HEADER_SIZE {
            return Err(AotError::CorruptHeader);
        }
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[0..4]);
        let mut source_sha1 = [0u8; 20];
        source_sha1.copy_from_slice(&bytes[16..36]);
        let mut reserved = [0u8; 8];
        reserved.copy_from_slice(&bytes[56..64]);

        Ok(Self {
            magic,
            format_version: read_u32_le(bytes, 4),
            arch: read_u16_le(bytes, 8),
            flags: read_u16_le(bytes, 10),
            section_count: read_u32_le(bytes, 12),
            source_sha1,
            entry_guest_va: read_u32_le(bytes, 36),
            image_base: read_u32_le(bytes, 40),
            payload_crc32: read_u32_le(bytes, 44),
            total_file_size: read_u64_le(bytes, 48),
            reserved,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SectionEntry {
    pub kind: u32,
    pub flags: u32,
    pub file_offset: u64,
    pub byte_len: u64,
    pub entry_count: u32,
    pub crc32: u32,
}

impl SectionEntry {
    pub fn to_bytes(&self) -> [u8; AOT_SECTION_ENTRY_SIZE] {
        let mut buf = [0u8; AOT_SECTION_ENTRY_SIZE];
        buf[0..4].copy_from_slice(&self.kind.to_le_bytes());
        buf[4..8].copy_from_slice(&self.flags.to_le_bytes());
        buf[8..16].copy_from_slice(&self.file_offset.to_le_bytes());
        buf[16..24].copy_from_slice(&self.byte_len.to_le_bytes());
        buf[24..28].copy_from_slice(&self.entry_count.to_le_bytes());
        buf[28..32].copy_from_slice(&self.crc32.to_le_bytes());
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AotError> {
        if bytes.len() != AOT_SECTION_ENTRY_SIZE {
            return Err(AotError::InvalidSection);
        }
        Ok(Self {
            kind: read_u32_le(bytes, 0),
            flags: read_u32_le(bytes, 4),
            file_offset: read_u64_le(bytes, 8),
            byte_len: read_u64_le(bytes, 16),
            entry_count: read_u32_le(bytes, 24),
            crc32: read_u32_le(bytes, 28),
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DispatchEntry {
    pub guest_va: u32,
    pub code_offset: u32,
    pub code_size: u32,
    pub flags: u32,
}

impl DispatchEntry {
    pub fn to_bytes(&self) -> [u8; AOT_DISPATCH_ENTRY_SIZE] {
        let mut buf = [0u8; AOT_DISPATCH_ENTRY_SIZE];
        buf[0..4].copy_from_slice(&self.guest_va.to_le_bytes());
        buf[4..8].copy_from_slice(&self.code_offset.to_le_bytes());
        buf[8..12].copy_from_slice(&self.code_size.to_le_bytes());
        buf[12..16].copy_from_slice(&self.flags.to_le_bytes());
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AotError> {
        if bytes.len() != AOT_DISPATCH_ENTRY_SIZE {
            return Err(AotError::InvalidSection);
        }
        Ok(Self {
            guest_va: read_u32_le(bytes, 0),
            code_offset: read_u32_le(bytes, 4),
            code_size: read_u32_le(bytes, 8),
            flags: read_u32_le(bytes, 12),
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RelocEntry {
    pub code_offset: u32,
    pub target_va: u32,
    pub kind: u16,
    pub flags: u16,
    pub addend: i32,
}

impl RelocEntry {
    pub fn to_bytes(&self) -> [u8; AOT_RELOC_ENTRY_SIZE] {
        let mut buf = [0u8; AOT_RELOC_ENTRY_SIZE];
        buf[0..4].copy_from_slice(&self.code_offset.to_le_bytes());
        buf[4..8].copy_from_slice(&self.target_va.to_le_bytes());
        buf[8..10].copy_from_slice(&self.kind.to_le_bytes());
        buf[10..12].copy_from_slice(&self.flags.to_le_bytes());
        buf[12..16].copy_from_slice(&self.addend.to_le_bytes());
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AotError> {
        if bytes.len() != AOT_RELOC_ENTRY_SIZE {
            return Err(AotError::InvalidSection);
        }
        Ok(Self {
            code_offset: read_u32_le(bytes, 0),
            target_va: read_u32_le(bytes, 4),
            kind: read_u16_le(bytes, 8),
            flags: read_u16_le(bytes, 10),
            addend: i32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ImportEntry {
    pub iat_va: u32,
    pub dll_name_offset: u32,
    pub symbol_name_offset: u32,
    pub ordinal: u16,
    pub flags: u16,
}

impl ImportEntry {
    pub fn to_bytes(&self) -> [u8; AOT_IMPORT_ENTRY_SIZE] {
        let mut buf = [0u8; AOT_IMPORT_ENTRY_SIZE];
        buf[0..4].copy_from_slice(&self.iat_va.to_le_bytes());
        buf[4..8].copy_from_slice(&self.dll_name_offset.to_le_bytes());
        buf[8..12].copy_from_slice(&self.symbol_name_offset.to_le_bytes());
        buf[12..14].copy_from_slice(&self.ordinal.to_le_bytes());
        buf[14..16].copy_from_slice(&self.flags.to_le_bytes());
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AotError> {
        if bytes.len() != AOT_IMPORT_ENTRY_SIZE {
            return Err(AotError::InvalidSection);
        }
        Ok(Self {
            iat_va: read_u32_le(bytes, 0),
            dll_name_offset: read_u32_le(bytes, 4),
            symbol_name_offset: read_u32_le(bytes, 8),
            ordinal: read_u16_le(bytes, 12),
            flags: read_u16_le(bytes, 14),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportSymbol {
    Name(String),
    Ordinal(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedImport {
    pub iat_va: u32,
    pub dll_name: String,
    pub symbol: ImportSymbol,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AotBuildStats {
    pub function_count: usize,
    pub code_bytes: u64,
    pub reloc_count: usize,
    pub import_count: usize,
    pub total_file_size: u64,
}

#[derive(Debug)]
pub enum AotError {
    Io(std::io::Error),
    InvalidMagic,
    UnsupportedVersion(u32),
    UnsupportedArch(u16),
    CorruptHeader,
    ChecksumMismatch {
        section: &'static str,
        expected: u32,
        actual: u32,
    },
    InvalidSection,
    InvalidStringTable,
    DuplicateFunction(u32),
    UnresolvedRelocation(u32),
    RelocationOutOfRange {
        code_offset: u32,
        target_va: u32,
    },
}

impl fmt::Display for AotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::InvalidMagic => write!(f, "invalid AOT file magic"),
            Self::UnsupportedVersion(ver) => write!(f, "unsupported AOT format version: {ver}"),
            Self::UnsupportedArch(arch) => write!(f, "unsupported AOT target architecture: {arch}"),
            Self::CorruptHeader => write!(f, "corrupt AOT header"),
            Self::ChecksumMismatch {
                section,
                expected,
                actual,
            } => write!(
                f,
                "checksum mismatch in {section} section: expected 0x{expected:08x}, got 0x{actual:08x}"
            ),
            Self::InvalidSection => write!(f, "invalid or out-of-bounds AOT section"),
            Self::InvalidStringTable => write!(f, "invalid AOT string table entry"),
            Self::DuplicateFunction(va) => write!(f, "duplicate function VA: 0x{va:08x}"),
            Self::UnresolvedRelocation(va) => {
                write!(f, "unresolved internal relocation target VA: 0x{va:08x}")
            }
            Self::RelocationOutOfRange {
                code_offset,
                target_va,
            } => write!(
                f,
                "relocation out of range at code offset 0x{code_offset:08x} targeting 0x{target_va:08x}"
            ),
        }
    }
}

impl std::error::Error for AotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AotError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

pub struct AotWriter<W: Write + Seek> {
    writer: W,
    arch: TargetArch,
    source_sha1: [u8; 20],
    entry_guest_va: u32,
    image_base: u32,
    code_bytes_written: u64,
    code_crc32: u32,
    dispatch: Vec<DispatchEntry>,
    relocations: Vec<RelocEntry>,
    imports: Vec<ImportEntry>,
    strings: Vec<u8>,
    string_intern: HashMap<String, u32>,
}

impl<W: Write + Seek> AotWriter<W> {
    pub fn new(
        mut writer: W,
        arch: TargetArch,
        source_sha1: [u8; 20],
        entry_guest_va: u32,
        image_base: u32,
    ) -> Result<Self, AotError> {
        writer.seek(SeekFrom::Start(0))?;
        write_zero_padding(&mut writer, AOT_CODE_ALIGNMENT)?;

        Ok(Self {
            writer,
            arch,
            source_sha1,
            entry_guest_va,
            image_base,
            code_bytes_written: 0,
            code_crc32: 0,
            dispatch: Vec::new(),
            relocations: Vec::new(),
            imports: Vec::new(),
            strings: Vec::new(),
            string_intern: HashMap::new(),
        })
    }

    pub fn write_function(
        &mut self,
        guest_va: u32,
        flags: u32,
        code: &[u8],
    ) -> Result<u32, AotError> {
        let align_mask = (AOT_FUNC_ALIGNMENT as u64) - 1;
        let misalign = self.code_bytes_written & align_mask;
        if misalign != 0 {
            let pad_len = (AOT_FUNC_ALIGNMENT as u64 - misalign) as usize;
            let pad_buf = [self.arch.padding_byte(); AOT_FUNC_ALIGNMENT];
            self.write_code_chunk(&pad_buf[..pad_len])?;
        }

        let code_offset =
            u32::try_from(self.code_bytes_written).map_err(|_| AotError::InvalidSection)?;
        let code_size = u32::try_from(code.len()).map_err(|_| AotError::InvalidSection)?;

        self.write_code_chunk(code)?;
        self.dispatch.push(DispatchEntry {
            guest_va,
            code_offset,
            code_size,
            flags,
        });
        Ok(code_offset)
    }

    pub fn add_relocation(&mut self, reloc: RelocEntry) {
        self.relocations.push(reloc);
    }

    pub fn add_import(&mut self, iat_va: u32, dll_name: &str, symbol: ImportSymbol) {
        let dll_name_offset = self.intern_string(dll_name);
        let (symbol_name_offset, ordinal, flags) = match symbol {
            ImportSymbol::Name(name) => (self.intern_string(&name), 0, 0),
            ImportSymbol::Ordinal(ordinal) => {
                (IMPORT_NO_NAME_OFFSET, ordinal, IMPORT_BY_ORDINAL_FLAG)
            }
        };

        self.imports.push(ImportEntry {
            iat_va,
            dll_name_offset,
            symbol_name_offset,
            ordinal,
            flags,
        });
    }

    pub fn finish(mut self) -> Result<AotBuildStats, AotError> {
        self.dispatch.sort_unstable_by_key(|entry| entry.guest_va);
        for pair in self.dispatch.windows(2) {
            if pair[0].guest_va == pair[1].guest_va {
                return Err(AotError::DuplicateFunction(pair[0].guest_va));
            }
        }

        let code_section = SectionEntry {
            kind: SectionKind::Code as u32,
            flags: 0,
            file_offset: AOT_CODE_ALIGNMENT,
            byte_len: self.code_bytes_written,
            entry_count: u32::try_from(self.dispatch.len())
                .map_err(|_| AotError::InvalidSection)?,
            crc32: self.code_crc32,
        };

        let dispatch_bytes = serialize_entries(&self.dispatch, DispatchEntry::to_bytes);
        let dispatch_section = write_metadata_section(
            &mut self.writer,
            SectionKind::Dispatch,
            &dispatch_bytes,
            self.dispatch.len(),
        )?;

        let reloc_bytes = serialize_entries(&self.relocations, RelocEntry::to_bytes);
        let reloc_section = write_metadata_section(
            &mut self.writer,
            SectionKind::Relocations,
            &reloc_bytes,
            self.relocations.len(),
        )?;

        let import_bytes = serialize_entries(&self.imports, ImportEntry::to_bytes);
        let import_section = write_metadata_section(
            &mut self.writer,
            SectionKind::Imports,
            &import_bytes,
            self.imports.len(),
        )?;

        let string_section = write_metadata_section(
            &mut self.writer,
            SectionKind::Strings,
            &self.strings,
            self.strings.len(),
        )?;

        let total_file_size = self.writer.stream_position()?;

        let sections = [
            code_section,
            dispatch_section,
            reloc_section,
            import_section,
            string_section,
        ];
        let dir_bytes = serialize_entries(&sections, SectionEntry::to_bytes);
        let payload_crc32 = crc32(&dir_bytes);

        let header = AotHeader {
            magic: AOT_MAGIC,
            format_version: AOT_FORMAT_VERSION,
            arch: self.arch as u16,
            flags: 0,
            section_count: EXPECTED_SECTION_COUNT as u32,
            source_sha1: self.source_sha1,
            entry_guest_va: self.entry_guest_va,
            image_base: self.image_base,
            payload_crc32,
            total_file_size,
            reserved: [0u8; 8],
        };

        self.writer.seek(SeekFrom::Start(0))?;
        self.writer.write_all(&header.to_bytes())?;
        self.writer.write_all(&dir_bytes)?;
        self.writer.flush()?;

        Ok(AotBuildStats {
            function_count: self.dispatch.len(),
            code_bytes: self.code_bytes_written,
            reloc_count: self.relocations.len(),
            import_count: self.imports.len(),
            total_file_size,
        })
    }

    fn write_code_chunk(&mut self, bytes: &[u8]) -> Result<(), AotError> {
        self.writer.write_all(bytes)?;
        self.code_crc32 = crc32_update(self.code_crc32, bytes);
        self.code_bytes_written = self
            .code_bytes_written
            .checked_add(bytes.len() as u64)
            .ok_or(AotError::InvalidSection)?;
        Ok(())
    }

    fn intern_string(&mut self, text: &str) -> u32 {
        if let Some(&offset) = self.string_intern.get(text) {
            return offset;
        }
        let offset = self.strings.len() as u32;
        self.strings.extend_from_slice(text.as_bytes());
        self.strings.push(0);
        self.string_intern.insert(text.to_owned(), offset);
        offset
    }
}

impl<W: Read + Write + Seek> AotWriter<W> {
    pub fn patch_internal_relocations(&mut self) -> Result<usize, AotError> {
        let mut va_to_offset = HashMap::with_capacity(self.dispatch.len());
        for entry in &self.dispatch {
            va_to_offset.insert(entry.guest_va, entry.code_offset);
        }

        let mut patched = 0usize;
        for reloc in &self.relocations {
            let Some(kind) = RelocKind::from_u16(reloc.kind) else {
                continue;
            };
            if !matches!(kind, RelocKind::X86_64Rel32 | RelocKind::AArch64Call26) {
                continue;
            }

            let target_offset = *va_to_offset
                .get(&reloc.target_va)
                .ok_or(AotError::UnresolvedRelocation(reloc.target_va))?;

            let end_offset = (reloc.code_offset as u64)
                .checked_add(4)
                .ok_or(AotError::RelocationOutOfRange {
                    code_offset: reloc.code_offset,
                    target_va: reloc.target_va,
                })?;
            if end_offset > self.code_bytes_written {
                return Err(AotError::RelocationOutOfRange {
                    code_offset: reloc.code_offset,
                    target_va: reloc.target_va,
                });
            }

            let file_pos = AOT_CODE_ALIGNMENT + reloc.code_offset as u64;
            match kind {
                RelocKind::X86_64Rel32 => {
                    let disp = (target_offset as i64)
                        - (reloc.code_offset as i64)
                        + (reloc.addend as i64);
                    let disp32 =
                        i32::try_from(disp).map_err(|_| AotError::RelocationOutOfRange {
                            code_offset: reloc.code_offset,
                            target_va: reloc.target_va,
                        })?;
                    self.writer.seek(SeekFrom::Start(file_pos))?;
                    self.writer.write_all(&disp32.to_le_bytes())?;
                }
                RelocKind::AArch64Call26 => {
                    self.writer.seek(SeekFrom::Start(file_pos))?;
                    let mut word_bytes = [0u8; 4];
                    self.writer.read_exact(&mut word_bytes)?;
                    let existing = u32::from_le_bytes(word_bytes);

                    let byte_delta = (target_offset as i64)
                        - (reloc.code_offset as i64)
                        + (reloc.addend as i64);
                    if (byte_delta & 3) != 0
                        || !(-AARCH64_CALL26_RANGE..AARCH64_CALL26_RANGE).contains(&byte_delta)
                    {
                        return Err(AotError::RelocationOutOfRange {
                            code_offset: reloc.code_offset,
                            target_va: reloc.target_va,
                        });
                    }

                    let imm26 = ((byte_delta >> 2) as u32) & AARCH64_CALL26_IMM_MASK;
                    let updated = (existing & AARCH64_CALL26_OP_MASK) | imm26;
                    self.writer.seek(SeekFrom::Start(file_pos))?;
                    self.writer.write_all(&updated.to_le_bytes())?;
                }
                RelocKind::GotSlot32 => unreachable!(),
            }
            patched += 1;
        }

        if patched > 0 {
            self.writer.seek(SeekFrom::Start(AOT_CODE_ALIGNMENT))?;
            let mut remaining = self.code_bytes_written;
            let mut chunk = vec![0u8; CRC_STREAM_CHUNK_SIZE];
            let mut running_crc = 0u32;
            while remaining > 0 {
                let to_read = remaining.min(CRC_STREAM_CHUNK_SIZE as u64) as usize;
                self.writer.read_exact(&mut chunk[..to_read])?;
                running_crc = crc32_update(running_crc, &chunk[..to_read]);
                remaining -= to_read as u64;
            }
            self.code_crc32 = running_crc;
            self.writer
                .seek(SeekFrom::Start(AOT_CODE_ALIGNMENT + self.code_bytes_written))?;
        }

        Ok(patched)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AotManifest {
    pub header: AotHeader,
    pub code_section: SectionEntry,
    pub dispatch: Vec<DispatchEntry>,
    pub relocations: Vec<RelocEntry>,
    pub imports: Vec<ResolvedImport>,
}

impl AotManifest {
    pub fn read_from<R: Read + Seek>(
        reader: &mut R,
        verify_code_crc: bool,
    ) -> Result<Self, AotError> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        if file_len < AOT_HEADER_SIZE as u64 {
            return Err(AotError::CorruptHeader);
        }

        reader.seek(SeekFrom::Start(0))?;
        let mut hdr_buf = [0u8; AOT_HEADER_SIZE];
        reader.read_exact(&mut hdr_buf)?;
        let header = AotHeader::from_bytes(&hdr_buf)?;

        validate_header(&header, file_len)?;

        let dir_byte_len = (header.section_count as usize) * AOT_SECTION_ENTRY_SIZE;
        let mut dir_bytes = vec![0u8; dir_byte_len];
        reader.read_exact(&mut dir_bytes)?;

        let actual_dir_crc = crc32(&dir_bytes);
        if actual_dir_crc != header.payload_crc32 {
            return Err(AotError::ChecksumMismatch {
                section: "section_directory",
                expected: header.payload_crc32,
                actual: actual_dir_crc,
            });
        }

        let sections = parse_sections(&dir_bytes, file_len)?;
        let code_section = sections.code;

        let dispatch_bytes = read_verified_section(reader, SectionKind::Dispatch, &sections.dispatch)?;
        let dispatch = parse_fixed_entries(
            &dispatch_bytes,
            AOT_DISPATCH_ENTRY_SIZE,
            sections.dispatch.entry_count,
            DispatchEntry::from_bytes,
        )?;
        validate_dispatch_order(&dispatch)?;

        let reloc_bytes =
            read_verified_section(reader, SectionKind::Relocations, &sections.relocations)?;
        let relocations = parse_fixed_entries(
            &reloc_bytes,
            AOT_RELOC_ENTRY_SIZE,
            sections.relocations.entry_count,
            RelocEntry::from_bytes,
        )?;

        let import_bytes = read_verified_section(reader, SectionKind::Imports, &sections.imports)?;
        let raw_imports = parse_fixed_entries(
            &import_bytes,
            AOT_IMPORT_ENTRY_SIZE,
            sections.imports.entry_count,
            ImportEntry::from_bytes,
        )?;

        let strings = read_verified_section(reader, SectionKind::Strings, &sections.strings)?;
        let imports = resolve_imports(&raw_imports, &strings)?;

        if verify_code_crc {
            verify_section_crc_streaming(reader, &code_section)?;
        }

        Ok(Self {
            header,
            code_section,
            dispatch,
            relocations,
            imports,
        })
    }

    pub fn lookup_va(&self, guest_va: u32) -> Option<&DispatchEntry> {
        self.dispatch
            .binary_search_by_key(&guest_va, |entry| entry.guest_va)
            .ok()
            .map(|idx| &self.dispatch[idx])
    }
}

struct ParsedSections {
    code: SectionEntry,
    dispatch: SectionEntry,
    relocations: SectionEntry,
    imports: SectionEntry,
    strings: SectionEntry,
}

fn validate_header(header: &AotHeader, file_len: u64) -> Result<(), AotError> {
    if header.magic != AOT_MAGIC {
        return Err(AotError::InvalidMagic);
    }
    if header.format_version != AOT_FORMAT_VERSION {
        return Err(AotError::UnsupportedVersion(header.format_version));
    }
    if TargetArch::from_u16(header.arch).is_none() {
        return Err(AotError::UnsupportedArch(header.arch));
    }
    if header.total_file_size != file_len {
        return Err(AotError::CorruptHeader);
    }

    let dir_bytes = (header.section_count as u64)
        .checked_mul(AOT_SECTION_ENTRY_SIZE as u64)
        .ok_or(AotError::CorruptHeader)?;
    let dir_end = (AOT_HEADER_SIZE as u64)
        .checked_add(dir_bytes)
        .ok_or(AotError::CorruptHeader)?;
    if dir_end > file_len || (header.section_count as usize) < EXPECTED_SECTION_COUNT {
        return Err(AotError::CorruptHeader);
    }
    Ok(())
}

fn parse_sections(dir_bytes: &[u8], file_len: u64) -> Result<ParsedSections, AotError> {
    let mut code = None;
    let mut dispatch = None;
    let mut relocations = None;
    let mut imports = None;
    let mut strings = None;

    for chunk in dir_bytes.chunks_exact(AOT_SECTION_ENTRY_SIZE) {
        let entry = SectionEntry::from_bytes(chunk)?;
        let end = entry
            .file_offset
            .checked_add(entry.byte_len)
            .ok_or(AotError::InvalidSection)?;
        if end > file_len {
            return Err(AotError::InvalidSection);
        }

        let kind = SectionKind::from_u32(entry.kind).ok_or(AotError::InvalidSection)?;
        let slot = match kind {
            SectionKind::Code => &mut code,
            SectionKind::Dispatch => &mut dispatch,
            SectionKind::Relocations => &mut relocations,
            SectionKind::Imports => &mut imports,
            SectionKind::Strings => &mut strings,
        };
        if slot.is_some() {
            return Err(AotError::InvalidSection);
        }
        *slot = Some(entry);
    }

    Ok(ParsedSections {
        code: code.ok_or(AotError::InvalidSection)?,
        dispatch: dispatch.ok_or(AotError::InvalidSection)?,
        relocations: relocations.ok_or(AotError::InvalidSection)?,
        imports: imports.ok_or(AotError::InvalidSection)?,
        strings: strings.ok_or(AotError::InvalidSection)?,
    })
}

fn read_verified_section<R: Read + Seek>(
    reader: &mut R,
    kind: SectionKind,
    section: &SectionEntry,
) -> Result<Vec<u8>, AotError> {
    let byte_len = usize::try_from(section.byte_len).map_err(|_| AotError::InvalidSection)?;
    let mut buf = vec![0u8; byte_len];
    if byte_len > 0 {
        reader.seek(SeekFrom::Start(section.file_offset))?;
        reader.read_exact(&mut buf)?;
    }
    let actual = crc32(&buf);
    if actual != section.crc32 {
        return Err(AotError::ChecksumMismatch {
            section: kind.name(),
            expected: section.crc32,
            actual,
        });
    }
    Ok(buf)
}

fn verify_section_crc_streaming<R: Read + Seek>(
    reader: &mut R,
    section: &SectionEntry,
) -> Result<(), AotError> {
    reader.seek(SeekFrom::Start(section.file_offset))?;
    let mut remaining = section.byte_len;
    let mut chunk = vec![0u8; CRC_STREAM_CHUNK_SIZE];
    let mut running_crc = 0u32;

    while remaining > 0 {
        let to_read = remaining.min(CRC_STREAM_CHUNK_SIZE as u64) as usize;
        reader.read_exact(&mut chunk[..to_read])?;
        running_crc = crc32_update(running_crc, &chunk[..to_read]);
        remaining -= to_read as u64;
    }

    if running_crc != section.crc32 {
        return Err(AotError::ChecksumMismatch {
            section: SectionKind::Code.name(),
            expected: section.crc32,
            actual: running_crc,
        });
    }
    Ok(())
}

fn parse_fixed_entries<T>(
    bytes: &[u8],
    entry_size: usize,
    expected_count: u32,
    parser: fn(&[u8]) -> Result<T, AotError>,
) -> Result<Vec<T>, AotError> {
    let expected_bytes = (expected_count as usize)
        .checked_mul(entry_size)
        .ok_or(AotError::InvalidSection)?;
    if bytes.len() != expected_bytes {
        return Err(AotError::InvalidSection);
    }

    let mut entries = Vec::with_capacity(expected_count as usize);
    for chunk in bytes.chunks_exact(entry_size) {
        entries.push(parser(chunk)?);
    }
    Ok(entries)
}

fn validate_dispatch_order(dispatch: &[DispatchEntry]) -> Result<(), AotError> {
    for pair in dispatch.windows(2) {
        if pair[0].guest_va == pair[1].guest_va {
            return Err(AotError::DuplicateFunction(pair[0].guest_va));
        }
        if pair[0].guest_va > pair[1].guest_va {
            return Err(AotError::InvalidSection);
        }
    }
    Ok(())
}

fn resolve_imports(raw: &[ImportEntry], strings: &[u8]) -> Result<Vec<ResolvedImport>, AotError> {
    let mut resolved = Vec::with_capacity(raw.len());
    for entry in raw {
        let dll_name = read_cstr(strings, entry.dll_name_offset)?;
        let symbol = if (entry.flags & IMPORT_BY_ORDINAL_FLAG) != 0 {
            ImportSymbol::Ordinal(entry.ordinal)
        } else {
            if entry.symbol_name_offset == IMPORT_NO_NAME_OFFSET {
                return Err(AotError::InvalidStringTable);
            }
            ImportSymbol::Name(read_cstr(strings, entry.symbol_name_offset)?)
        };
        resolved.push(ResolvedImport {
            iat_va: entry.iat_va,
            dll_name,
            symbol,
        });
    }
    Ok(resolved)
}

fn read_cstr(strings: &[u8], offset: u32) -> Result<String, AotError> {
    let start = offset as usize;
    if start >= strings.len() {
        return Err(AotError::InvalidStringTable);
    }
    let tail = &strings[start..];
    let rel_end = tail
        .iter()
        .position(|&byte| byte == 0)
        .ok_or(AotError::InvalidStringTable)?;
    let text = std::str::from_utf8(&tail[..rel_end]).map_err(|_| AotError::InvalidStringTable)?;
    Ok(text.to_owned())
}

fn serialize_entries<T, const N: usize>(entries: &[T], serializer: fn(&T) -> [u8; N]) -> Vec<u8> {
    let mut out = Vec::with_capacity(entries.len() * N);
    for entry in entries {
        out.extend_from_slice(&serializer(entry));
    }
    out
}

fn write_zero_padding<W: Write>(writer: &mut W, mut total_bytes: u64) -> Result<(), AotError> {
    let zeros = [0u8; HEADER_PAD_CHUNK_SIZE];
    while total_bytes > 0 {
        let chunk = total_bytes.min(HEADER_PAD_CHUNK_SIZE as u64) as usize;
        writer.write_all(&zeros[..chunk])?;
        total_bytes -= chunk as u64;
    }
    Ok(())
}

fn write_metadata_section<W: Write + Seek>(
    writer: &mut W,
    kind: SectionKind,
    bytes: &[u8],
    entry_count: usize,
) -> Result<SectionEntry, AotError> {
    let file_offset = writer.stream_position()?;
    writer.write_all(bytes)?;
    Ok(SectionEntry {
        kind: kind as u32,
        flags: 0,
        file_offset,
        byte_len: bytes.len() as u64,
        entry_count: u32::try_from(entry_count).map_err(|_| AotError::InvalidSection)?,
        crc32: crc32(bytes),
    })
}

const fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

const fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

const fn read_u64_le(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn crc32_standard_vector_and_streaming() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let partial = crc32_update(0, b"1234");
        assert_eq!(crc32_update(partial, b"56789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn serialized_record_sizes_match_constants() {
        let header = AotHeader {
            magic: AOT_MAGIC,
            format_version: AOT_FORMAT_VERSION,
            arch: TargetArch::X86_64 as u16,
            flags: 0,
            section_count: 5,
            source_sha1: [0xAB; 20],
            entry_guest_va: 0x0040_1000,
            image_base: 0x0040_0000,
            payload_crc32: 0x1234_5678,
            total_file_size: 0x20000,
            reserved: [0; 8],
        };
        assert_eq!(
            AotHeader::from_bytes(&header.to_bytes()).unwrap(),
            header
        );

        let sec = SectionEntry {
            kind: SectionKind::Code as u32,
            flags: 1,
            file_offset: AOT_CODE_ALIGNMENT,
            byte_len: 128,
            entry_count: 2,
            crc32: 0xDEAD_BEEF,
        };
        assert_eq!(SectionEntry::from_bytes(&sec.to_bytes()).unwrap(), sec);

        let disp = DispatchEntry {
            guest_va: 0x0040_2000,
            code_offset: 16,
            code_size: 42,
            flags: FUNC_FLAG_ENTRY_POINT,
        };
        assert_eq!(DispatchEntry::from_bytes(&disp.to_bytes()).unwrap(), disp);

        let reloc = RelocEntry {
            code_offset: 8,
            target_va: 0x0040_3000,
            kind: RelocKind::X86_64Rel32 as u16,
            flags: 0,
            addend: -4,
        };
        assert_eq!(RelocEntry::from_bytes(&reloc.to_bytes()).unwrap(), reloc);

        let imp = ImportEntry {
            iat_va: 0x0060_1000,
            dll_name_offset: 0,
            symbol_name_offset: 13,
            ordinal: 0,
            flags: 0,
        };
        assert_eq!(ImportEntry::from_bytes(&imp.to_bytes()).unwrap(), imp);
    }

    #[test]
    fn writer_reader_round_trip_and_lookup() {
        let mut cursor = Cursor::new(Vec::new());
        let sha1 = [0x42u8; 20];
        let mut writer =
            AotWriter::new(&mut cursor, TargetArch::AArch64, sha1, 0x0040_1000, 0x0040_0000)
                .unwrap();

        // Write intentionally out-of-VA-order to verify sorting in finish()
        let off_b = writer
            .write_function(0x0040_2000, FUNC_FLAG_JUMP_TABLE_TARGET, &[1, 2, 3, 4, 5])
            .unwrap();
        let off_a = writer
            .write_function(0x0040_1000, FUNC_FLAG_ENTRY_POINT, &[0xAA; 16])
            .unwrap();
        assert_eq!(off_b, 0);
        assert_eq!(off_a, 16);

        writer.add_relocation(RelocEntry {
            code_offset: 4,
            target_va: 0x0040_1000,
            kind: RelocKind::AArch64Call26 as u16,
            flags: 0,
            addend: 0,
        });
        writer.add_import(
            0x0060_0000,
            "KERNEL32.dll",
            ImportSymbol::Name("CreateFileA".to_owned()),
        );
        writer.add_import(0x0060_0004, "USER32.dll", ImportSymbol::Ordinal(123));

        let stats = writer.finish().unwrap();
        assert_eq!(stats.function_count, 2);
        assert_eq!(stats.code_bytes, 32);
        assert_eq!(stats.reloc_count, 1);
        assert_eq!(stats.import_count, 2);

        cursor.set_position(0);
        let manifest = AotManifest::read_from(&mut cursor, true).unwrap();
        assert_eq!(manifest.header.source_sha1, sha1);
        assert_eq!(manifest.code_section.file_offset, AOT_CODE_ALIGNMENT);
        assert_eq!(manifest.code_section.file_offset % AOT_CODE_ALIGNMENT, 0);

        let fn_a = manifest.lookup_va(0x0040_1000).unwrap();
        assert_eq!(fn_a.code_offset, 16);
        assert_eq!(fn_a.code_size, 16);
        assert_eq!(fn_a.flags, FUNC_FLAG_ENTRY_POINT);

        let fn_b = manifest.lookup_va(0x0040_2000).unwrap();
        assert_eq!(fn_b.code_offset, 0);
        assert_eq!(fn_b.code_size, 5);

        assert!(manifest.lookup_va(0x0040_1500).is_none());
        assert_eq!(manifest.relocations.len(), 1);
        assert_eq!(manifest.imports.len(), 2);
        assert_eq!(manifest.imports[0].dll_name, "KERNEL32.dll");
        assert_eq!(
            manifest.imports[0].symbol,
            ImportSymbol::Name("CreateFileA".to_owned())
        );
        assert_eq!(manifest.imports[1].dll_name, "USER32.dll");
        assert_eq!(manifest.imports[1].symbol, ImportSymbol::Ordinal(123));
    }

    #[test]
    fn duplicate_function_is_rejected() {
        let mut cursor = Cursor::new(Vec::new());
        let mut writer = AotWriter::new(
            &mut cursor,
            TargetArch::X86_64,
            [0; 20],
            0x0040_1000,
            0x0040_0000,
        )
        .unwrap();
        writer.write_function(0x0040_1000, 0, &[0xC3]).unwrap();
        writer.write_function(0x0040_1000, 0, &[0x90, 0xC3]).unwrap();
        let err = writer.finish().unwrap_err();
        assert!(matches!(err, AotError::DuplicateFunction(0x0040_1000)));
    }

    #[test]
    fn corrupt_code_byte_triggers_checksum_mismatch() {
        let mut cursor = Cursor::new(Vec::new());
        let mut writer = AotWriter::new(
            &mut cursor,
            TargetArch::X86_64,
            [0; 20],
            0x0040_1000,
            0x0040_0000,
        )
        .unwrap();
        writer
            .write_function(0x0040_1000, FUNC_FLAG_ENTRY_POINT, &[0x90, 0xC3])
            .unwrap();
        writer.finish().unwrap();

        let mut bytes = cursor.into_inner();
        bytes[AOT_CODE_ALIGNMENT as usize] ^= 0x01;

        let mut corrupted = Cursor::new(bytes);
        let err = AotManifest::read_from(&mut corrupted, true).unwrap_err();
        assert!(matches!(
            err,
            AotError::ChecksumMismatch {
                section: "code",
                ..
            }
        ));
    }

    #[test]
    fn patch_internal_relocations_x86_64_and_aarch64_with_crc() {
        // Test X86_64Rel32 forward and backward patching + full CRC verification
        let mut cursor_x86 = Cursor::new(Vec::new());
        let mut writer_x86 = AotWriter::new(
            &mut cursor_x86,
            TargetArch::X86_64,
            [1; 20],
            0x0040_1000,
            0x0040_0000,
        )
        .unwrap();

        // fn1 at offset 0: E8 <rel32 at +1> C3
        let off1 = writer_x86
            .write_function(0x0040_1000, FUNC_FLAG_ENTRY_POINT, &[0xE8, 0, 0, 0, 0, 0xC3])
            .unwrap();
        // fn2 at offset 16: E8 <rel32 at +17> C3
        let off2 = writer_x86
            .write_function(0x0040_2000, 0, &[0xE8, 0, 0, 0, 0, 0xC3])
            .unwrap();
        assert_eq!(off1, 0);
        assert_eq!(off2, 16);

        // Forward call fn1 -> fn2: disp = 16 - 1 - 4 = 11
        writer_x86.add_relocation(RelocEntry {
            code_offset: 1,
            target_va: 0x0040_2000,
            kind: RelocKind::X86_64Rel32 as u16,
            flags: 0,
            addend: -4,
        });
        // Backward call fn2 -> fn1: disp = 0 - 17 - 4 = -21
        writer_x86.add_relocation(RelocEntry {
            code_offset: 17,
            target_va: 0x0040_1000,
            kind: RelocKind::X86_64Rel32 as u16,
            flags: 0,
            addend: -4,
        });

        assert_eq!(writer_x86.patch_internal_relocations().unwrap(), 2);
        writer_x86.finish().unwrap();

        cursor_x86.set_position(0);
        let manifest_x86 = AotManifest::read_from(&mut cursor_x86, true).unwrap();
        assert_eq!(manifest_x86.relocations.len(), 2);

        let raw_x86 = cursor_x86.into_inner();
        let code_base = AOT_CODE_ALIGNMENT as usize;
        let disp_fwd = i32::from_le_bytes(raw_x86[code_base + 1..code_base + 5].try_into().unwrap());
        let disp_bwd =
            i32::from_le_bytes(raw_x86[code_base + 17..code_base + 21].try_into().unwrap());
        assert_eq!(disp_fwd, 11);
        assert_eq!(disp_bwd, -21);

        // Test AArch64Call26 forward and backward patching preserving upper 6 opcode bits (0x94000000 = BL)
        let mut cursor_arm = Cursor::new(Vec::new());
        let mut writer_arm = AotWriter::new(
            &mut cursor_arm,
            TargetArch::AArch64,
            [2; 20],
            0x0040_1000,
            0x0040_0000,
        )
        .unwrap();
        let bl_op = 0x9400_0000u32.to_le_bytes();
        let ret_op = 0xD65F_03C0u32.to_le_bytes();
        let mut fn_bytes = [0u8; 8];
        fn_bytes[0..4].copy_from_slice(&bl_op);
        fn_bytes[4..8].copy_from_slice(&ret_op);

        writer_arm
            .write_function(0x0040_1000, FUNC_FLAG_ENTRY_POINT, &fn_bytes)
            .unwrap();
        writer_arm.write_function(0x0040_2000, 0, &fn_bytes).unwrap();
        // Forward: offset 0 -> offset 16 (imm26 = +4)
        writer_arm.add_relocation(RelocEntry {
            code_offset: 0,
            target_va: 0x0040_2000,
            kind: RelocKind::AArch64Call26 as u16,
            flags: 0,
            addend: 0,
        });
        // Backward: offset 16 -> offset 0 (imm26 = -4 -> 0x03FF_FFFC)
        writer_arm.add_relocation(RelocEntry {
            code_offset: 16,
            target_va: 0x0040_1000,
            kind: RelocKind::AArch64Call26 as u16,
            flags: 0,
            addend: 0,
        });

        assert_eq!(writer_arm.patch_internal_relocations().unwrap(), 2);
        writer_arm.finish().unwrap();

        cursor_arm.set_position(0);
        AotManifest::read_from(&mut cursor_arm, true).unwrap();
        let raw_arm = cursor_arm.into_inner();
        let insn_fwd = u32::from_le_bytes(raw_arm[code_base..code_base + 4].try_into().unwrap());
        let insn_bwd =
            u32::from_le_bytes(raw_arm[code_base + 16..code_base + 20].try_into().unwrap());
        assert_eq!(insn_fwd, 0x9400_0004);
        assert_eq!(insn_bwd, 0x97FF_FFFC);
    }

    #[test]
    fn patch_internal_relocations_rejects_unresolved_and_out_of_range() {
        let mut cursor = Cursor::new(Vec::new());
        let mut writer = AotWriter::new(
            &mut cursor,
            TargetArch::X86_64,
            [0; 20],
            0x0040_1000,
            0x0040_0000,
        )
        .unwrap();
        writer
            .write_function(0x0040_1000, 0, &[0xE8, 0, 0, 0, 0, 0xC3])
            .unwrap();
        writer.add_relocation(RelocEntry {
            code_offset: 1,
            target_va: 0x0040_9999,
            kind: RelocKind::X86_64Rel32 as u16,
            flags: 0,
            addend: -4,
        });
        assert!(matches!(
            writer.patch_internal_relocations(),
            Err(AotError::UnresolvedRelocation(0x0040_9999))
        ));

        let mut cursor2 = Cursor::new(Vec::new());
        let mut writer2 = AotWriter::new(
            &mut cursor2,
            TargetArch::AArch64,
            [0; 20],
            0x0040_1000,
            0x0040_0000,
        )
        .unwrap();
        writer2
            .write_function(0x0040_1000, 0, &[0, 0, 0, 0x94])
            .unwrap();
        writer2.add_relocation(RelocEntry {
            code_offset: 100,
            target_va: 0x0040_1000,
            kind: RelocKind::AArch64Call26 as u16,
            flags: 0,
            addend: 0,
        });
        assert!(matches!(
            writer2.patch_internal_relocations(),
            Err(AotError::RelocationOutOfRange {
                code_offset: 100,
                target_va: 0x0040_1000
            })
        ));
    }
}
