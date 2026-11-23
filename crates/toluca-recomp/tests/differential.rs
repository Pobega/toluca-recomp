#![cfg(target_arch = "x86_64")]

use toluca_core::aot::TargetArch;
use toluca_core::bridge::{HostBridge, RecompiledFn, default_x87_helper};
use toluca_core::cpu::{
    EFLAGS_ARITH_MASK, EFLAGS_CF, EFLAGS_DF, EFLAGS_FIXED_ONE, EFLAGS_OF, EFLAGS_PF, EFLAGS_SF,
    EFLAGS_ZF, Gpr, GuestCpuState,
};
use toluca_recomp::lifter::{build_isa, lift_raw_snippet};
use unicorn_engine::unicorn_const::{Arch, Mode, Prot};
use unicorn_engine::{RegisterX86, Unicorn};

#[link(name = "atomic")]
unsafe extern "C" {}

const CODE_BASE: u32 = 0x0040_1000;
const CODE_SIZE: usize = 0x1_0000;
const SENTINEL_RET_VA: u32 = 0x0040_FFF0;

const DATA_BASE: u32 = 0x0001_0000;
const DATA_SIZE: usize = 0x1_0000;
const INITIAL_ESP: u32 = 0x0001_EFFC;
const ARENA_SIZE: usize = 0x0042_0000;
const MAX_EMU_STEPS: usize = 100_000;

const CR0_EM_BIT: u64 = 1 << 2;
const CR0_MP_BIT: u64 = 1 << 1;
const CR4_OSFXSR_BIT: u64 = 1 << 9;
const CR4_OSXMMEXCPT_BIT: u64 = 1 << 10;

struct ExecBuffer {
    ptr: *mut u8,
    len: usize,
}

impl ExecBuffer {
    fn new(code: &[u8]) -> Self {
        let len = code.len().max(4096);
        let ptr = unsafe {
            libc::mmap(
                core::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(ptr, libc::MAP_FAILED, "mmap failed");
        unsafe {
            core::ptr::copy_nonoverlapping(code.as_ptr(), ptr.cast::<u8>(), code.len());
            let rc = libc::mprotect(ptr, len, libc::PROT_READ | libc::PROT_EXEC);
            assert_eq!(rc, 0, "mprotect RX failed");
        }
        Self {
            ptr: ptr.cast::<u8>(),
            len,
        }
    }

    fn as_fn(&self) -> RecompiledFn {
        unsafe { core::mem::transmute::<*const u8, RecompiledFn>(self.ptr) }
    }
}

impl Drop for ExecBuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr.cast::<libc::c_void>(), self.len);
        }
    }
}

struct DifferentialCase<'a> {
    name: &'a str,
    code: &'a [u8],
    gpr: [u32; 8],
    initial_data: &'a [(u32, &'a [u8])],
    flags_mask: u32,
    check_xmm_count: usize,
}

impl<'a> DifferentialCase<'a> {
    fn new(name: &'a str, code: &'a [u8]) -> Self {
        let mut gpr = [0u32; 8];
        gpr[Gpr::Esp as usize] = INITIAL_ESP;
        Self {
            name,
            code,
            gpr,
            initial_data: &[],
            flags_mask: EFLAGS_ARITH_MASK | EFLAGS_DF,
            check_xmm_count: 0,
        }
    }

    fn with_gpr(mut self, reg: Gpr, val: u32) -> Self {
        self.gpr[reg as usize] = val;
        self
    }

    fn with_data(mut self, initial_data: &'a [(u32, &'a [u8])]) -> Self {
        self.initial_data = initial_data;
        self
    }

    fn with_flags_mask(mut self, mask: u32) -> Self {
        self.flags_mask = mask;
        self
    }

    fn with_xmm_check(mut self, count: usize) -> Self {
        self.check_xmm_count = count;
        self
    }
}

fn enable_unicorn_fpu_and_sse(emu: &mut Unicorn<'_, ()>) {
    let cr0 = emu.reg_read(RegisterX86::CR0).unwrap();
    let cr4 = emu.reg_read(RegisterX86::CR4).unwrap();
    emu.reg_write(RegisterX86::CR0, (cr0 & !CR0_EM_BIT) | CR0_MP_BIT)
        .unwrap();
    emu.reg_write(
        RegisterX86::CR4,
        cr4 | CR4_OSFXSR_BIT | CR4_OSXMMEXCPT_BIT,
    )
    .unwrap();
}

fn uc_gpr_reg(gpr: Gpr) -> RegisterX86 {
    match gpr {
        Gpr::Eax => RegisterX86::EAX,
        Gpr::Ecx => RegisterX86::ECX,
        Gpr::Edx => RegisterX86::EDX,
        Gpr::Ebx => RegisterX86::EBX,
        Gpr::Esp => RegisterX86::ESP,
        Gpr::Ebp => RegisterX86::EBP,
        Gpr::Esi => RegisterX86::ESI,
        Gpr::Edi => RegisterX86::EDI,
    }
}

fn uc_xmm_reg(idx: usize) -> RegisterX86 {
    match idx {
        0 => RegisterX86::XMM0,
        1 => RegisterX86::XMM1,
        2 => RegisterX86::XMM2,
        3 => RegisterX86::XMM3,
        _ => panic!("unsupported XMM check index {idx}"),
    }
}

fn assert_matches_unicorn(case: DifferentialCase<'_>) {
    let mut emu = Unicorn::new(Arch::X86, Mode::MODE_32).expect("unicorn init failed");
    enable_unicorn_fpu_and_sse(&mut emu);
    emu.mem_map(DATA_BASE as u64, DATA_SIZE as u64, Prot::ALL)
        .expect("unicorn data map failed");
    emu.mem_map(CODE_BASE as u64, CODE_SIZE as u64, Prot::ALL)
        .expect("unicorn code map failed");

    emu.mem_write(CODE_BASE as u64, case.code).unwrap();
    emu.mem_write(INITIAL_ESP as u64, &SENTINEL_RET_VA.to_le_bytes())
        .unwrap();
    for &(offset, bytes) in case.initial_data {
        emu.mem_write((DATA_BASE + offset) as u64, bytes).unwrap();
    }

    let all_gprs = [
        Gpr::Eax,
        Gpr::Ecx,
        Gpr::Edx,
        Gpr::Ebx,
        Gpr::Esp,
        Gpr::Ebp,
        Gpr::Esi,
        Gpr::Edi,
    ];
    for gpr in all_gprs {
        emu.reg_write(uc_gpr_reg(gpr), case.gpr[gpr as usize] as u64)
            .unwrap();
    }
    emu.reg_write(RegisterX86::EFLAGS, EFLAGS_FIXED_ONE as u64)
        .unwrap();

    emu.emu_start(
        CODE_BASE as u64,
        SENTINEL_RET_VA as u64,
        0,
        MAX_EMU_STEPS,
    )
    .unwrap_or_else(|err| panic!("[{}] unicorn execution failed: {err:?}", case.name));

    let uc_eip = emu.reg_read(RegisterX86::EIP).unwrap() as u32;
    let uc_eflags = emu.reg_read(RegisterX86::EFLAGS).unwrap() as u32;
    let mut uc_gprs = [0u32; 8];
    for gpr in all_gprs {
        uc_gprs[gpr as usize] = emu.reg_read(uc_gpr_reg(gpr)).unwrap() as u32;
    }
    let mut uc_data = vec![0u8; DATA_SIZE];
    emu.mem_read(DATA_BASE as u64, &mut uc_data).unwrap();

    let mut uc_xmms = [[0u8; 16]; 4];
    for (idx, slot) in uc_xmms.iter_mut().enumerate().take(case.check_xmm_count) {
        let raw = emu.reg_read_long(uc_xmm_reg(idx)).unwrap();
        slot.copy_from_slice(&raw[..16]);
    }

    let isa = build_isa(TargetArch::X86_64).unwrap();
    let compiled = lift_raw_snippet(isa.as_ref(), CODE_BASE, case.code)
        .unwrap_or_else(|err| panic!("[{}] lifter failed: {err}", case.name));
    assert_eq!(
        compiled.unsupported_count, 0,
        "[{}] encountered {} unsupported instructions",
        case.name, compiled.unsupported_count
    );

    let mut arena = vec![0u8; ARENA_SIZE];
    let code_start = CODE_BASE as usize;
    arena[code_start..code_start + case.code.len()].copy_from_slice(case.code);
    let esp_start = INITIAL_ESP as usize;
    arena[esp_start..esp_start + 4].copy_from_slice(&SENTINEL_RET_VA.to_le_bytes());
    for &(offset, bytes) in case.initial_data {
        let dst = (DATA_BASE + offset) as usize;
        arena[dst..dst + bytes.len()].copy_from_slice(bytes);
    }

    let mut bridge = HostBridge::new_stub();
    bridge.x87_helper = default_x87_helper;
    let mut cpu = GuestCpuState::new(arena.as_mut_ptr(), &bridge);
    cpu.gpr = case.gpr;
    cpu.eflags = EFLAGS_FIXED_ONE;

    let exec = ExecBuffer::new(&compiled.code);
    unsafe {
        (exec.as_fn())(&mut cpu);
    }
    cpu.materialize_flags();

    // 3. Assert bit-for-bit parity
    assert_eq!(
        cpu.eip, SENTINEL_RET_VA,
        "[{}] Cranelift EIP mismatch (expected sentinel 0x{SENTINEL_RET_VA:08x}, got 0x{:08x})",
        case.name, cpu.eip
    );
    assert_eq!(
        uc_eip, SENTINEL_RET_VA,
        "[{}] Unicorn EIP mismatch (expected sentinel 0x{SENTINEL_RET_VA:08x}, got 0x{uc_eip:08x})",
        case.name
    );

    for gpr in all_gprs {
        let got = cpu.gpr(gpr);
        let expected = uc_gprs[gpr as usize];
        assert_eq!(
            got, expected,
            "[{}] GPR {gpr:?} mismatch: Cranelift=0x{got:08x}, Unicorn=0x{expected:08x}",
            case.name
        );
    }

    let got_flags = cpu.eflags & case.flags_mask;
    let expected_flags = uc_eflags & case.flags_mask;
    assert_eq!(
        got_flags, expected_flags,
        "[{}] EFLAGS mismatch (mask=0x{:08x}): Cranelift=0x{got_flags:08x}, Unicorn=0x{expected_flags:08x}",
        case.name, case.flags_mask
    );

    let cr_data = &arena[DATA_BASE as usize..DATA_BASE as usize + DATA_SIZE];
    if cr_data != uc_data.as_slice() {
        let first_diff = cr_data
            .iter()
            .zip(uc_data.iter())
            .position(|(a, b)| a != b)
            .unwrap();
        panic!(
            "[{}] memory mismatch at DATA_BASE+0x{first_diff:04x}: Cranelift=0x{:02x}, Unicorn=0x{:02x}",
            case.name, cr_data[first_diff], uc_data[first_diff]
        );
    }

    for (idx, expected_xmm) in uc_xmms.iter().enumerate().take(case.check_xmm_count) {
        assert_eq!(
            &cpu.xmm[idx].0, expected_xmm,
            "[{}] XMM{idx} mismatch: Cranelift={:?}, Unicorn={:?}",
            case.name, cpu.xmm[idx].0, expected_xmm
        );
    }
}

#[test]
fn diff_integer_arithmetic_and_flags() {
    #[rustfmt::skip]
    let code = [
        0xB8, 0x78, 0x56, 0x34, 0x12, // mov eax, 0x12345678
        0x04, 0x90,                   // add al, 0x90 -> CF=1, OF=1, AF=0, SF=0, PF=0
        0xB4, 0xFE,                   // mov ah, 0xFE
        0x66, 0x83, 0xC0, 0x05,       // add ax, 5 -> wraps AX
        0x83, 0xD1, 0x0A,             // adc ecx, 10
        0x41,                         // inc ecx (must preserve CF)
        0x4A,                         // dec edx (must preserve CF)
        0x1B, 0xD3,                   // sbb edx, ebx
        0xF7, 0xDF,                   // neg edi
        0x39, 0xD8,                   // cmp eax, ebx
        0xC3,                         // ret
    ];
    assert_matches_unicorn(
        DifferentialCase::new("integer_arithmetic_and_flags", &code)
            .with_gpr(Gpr::Ecx, 0xFFFF_FFFF)
            .with_gpr(Gpr::Edx, 0x0000_1000)
            .with_gpr(Gpr::Ebx, 0x0000_0055)
            .with_gpr(Gpr::Edi, 0x1234),
    );

    // Explicitly verify INC/DEC preserve CF=1
    #[rustfmt::skip]
    let inc_cf = [
        0xF9,             // stc
        0xB8, 0xFF, 0x00, 0x00, 0x00, // mov eax, 0xFF
        0xFE, 0xC0,       // inc al (0xFF -> 0x00, CF stays 1)
        0x4A,             // dec edx (CF stays 1)
        0xC3,
    ];
    assert_matches_unicorn(
        DifferentialCase::new("inc_dec_preserve_cf", &inc_cf).with_gpr(Gpr::Edx, 0x8000_0000),
    );
}

#[test]
fn diff_mul_imul_div_idiv() {
    // MUL/IMUL only architecturally define CF and OF (SF/ZF/AF/PF are undefined).
    #[rustfmt::skip]
    let mul_code = [
        0xF7, 0xE1,             // mul ecx (EDX:EAX = EAX * ECX)
        0x8B, 0xF0,             // mov esi, eax
        0x8B, 0xFA,             // mov edi, edx
        0x69, 0xC3, 0x39, 0x05, 0x00, 0x00, // imul eax, ebx, 1337
        0x0F, 0xAF, 0xC6,       // imul eax, esi
        0xC3,
    ];
    assert_matches_unicorn(
        DifferentialCase::new("mul_and_imul", &mul_code)
            .with_gpr(Gpr::Eax, 0x1234_5678)
            .with_gpr(Gpr::Ecx, 0x9ABC_DEF0)
            .with_gpr(Gpr::Ebx, (-42i32) as u32)
            .with_flags_mask(EFLAGS_CF | EFLAGS_OF),
    );

    #[rustfmt::skip]
    let div_code = [
        0x33, 0xD2,             // xor edx, edx
        0xF7, 0xF1,             // div ecx -> EAX=quot, EDX=rem
        0x8B, 0xF0,             // mov esi, eax
        0x8B, 0xC3,             // mov eax, ebx
        0x99,                   // cdq
        0xF7, 0xFF,             // idiv edi -> EAX=signed quot, EDX=signed rem
        0xC3,
    ];
    assert_matches_unicorn(
        DifferentialCase::new("div_and_idiv", &div_code)
            .with_gpr(Gpr::Eax, 1_000_003)
            .with_gpr(Gpr::Ecx, 97)
            .with_gpr(Gpr::Ebx, (-123_456i32) as u32)
            .with_gpr(Gpr::Edi, 13)
            .with_flags_mask(0),
    );
}

#[test]
fn diff_bitwise_shifts_and_bit_ops() {
    // Shift by 0 must preserve prior flags; multi-bit shifts have undefined OF/AF.
    #[rustfmt::skip]
    let shift_zero = [
        0xF9,             // stc
        0xB1, 0x00,       // mov cl, 0
        0xD3, 0xE0,       // shl eax, cl
        0xD3, 0xE8,       // shr eax, cl
        0xD3, 0xF8,       // sar eax, cl
        0xC3,
    ];
    assert_matches_unicorn(
        DifferentialCase::new("shift_by_zero_preserves_flags", &shift_zero)
            .with_gpr(Gpr::Eax, 0x8000_0001),
    );

    #[rustfmt::skip]
    let shift_one = [
        0xD1, 0xE0,       // shl eax, 1
        0xD1, 0xE9,       // shr ecx, 1
        0xD1, 0xFA,       // sar edx, 1
        0xC3,
    ];
    assert_matches_unicorn(
        DifferentialCase::new("shift_by_one", &shift_one)
            .with_gpr(Gpr::Eax, 0x4000_0001)
            .with_gpr(Gpr::Ecx, 0x8000_0003)
            .with_gpr(Gpr::Edx, 0x8000_0005)
            .with_flags_mask(EFLAGS_CF | EFLAGS_PF | EFLAGS_ZF | EFLAGS_SF | EFLAGS_OF),
    );

    #[rustfmt::skip]
    let shift_multi = [
        0xC1, 0xE0, 0x05, // shl eax, 5
        0xC1, 0xE9, 0x1F, // shr ecx, 31
        0xC1, 0xFA, 0x1F, // sar edx, 31
        0xC3,
    ];
    assert_matches_unicorn(
        DifferentialCase::new("shift_multi_5_31", &shift_multi)
            .with_gpr(Gpr::Eax, 0x1234_5678)
            .with_gpr(Gpr::Ecx, 0x8000_0001)
            .with_gpr(Gpr::Edx, 0x8000_0000)
            .with_flags_mask(EFLAGS_CF | EFLAGS_PF | EFLAGS_ZF | EFLAGS_SF),
    );

    #[rustfmt::skip]
    let bit_ops = [
        0x23, 0xC3,       // and eax, ebx
        0x0B, 0xC1,       // or eax, ecx
        0x33, 0xC2,       // xor eax, edx
        0xF7, 0xD0,       // not eax
        0x85, 0xC3,       // test ebx, eax
        0x0F, 0xC8,       // bswap eax
        0x0F, 0xBA, 0xE8, 0x03, // bts eax, 3
        0x0F, 0xBA, 0xF0, 0x07, // btr eax, 7
        0x0F, 0xBA, 0xF8, 0x0B, // btc eax, 11
        0x0F, 0xBA, 0xE0, 0x03, // bt eax, 3
        0x0F, 0xBC, 0xF0,       // bsf esi, eax
        0x0F, 0xBD, 0xF8,       // bsr edi, eax
        0xC3,
    ];
    assert_matches_unicorn(
        DifferentialCase::new("bitwise_and_bit_ops", &bit_ops)
            .with_gpr(Gpr::Eax, 0x1122_3344)
            .with_gpr(Gpr::Ebx, 0xFF00_FF00)
            .with_gpr(Gpr::Ecx, 0x00AA_0055)
            .with_gpr(Gpr::Edx, 0xDEAD_BEEF)
            .with_flags_mask(EFLAGS_ZF),
    );

    #[rustfmt::skip]
    let rot_ops = [
        0xC1, 0xC0, 0x05, // rol eax, 5
        0xC1, 0xC9, 0x01, // ror ecx, 1
        0xC3,
    ];
    assert_matches_unicorn(
        DifferentialCase::new("rotate_rol_ror", &rot_ops)
            .with_gpr(Gpr::Eax, 0x1234_5678)
            .with_gpr(Gpr::Ecx, 0x8000_0001)
            .with_flags_mask(EFLAGS_CF | EFLAGS_OF),
    );
}

#[test]
fn diff_control_flow_loops_and_cmov() {
    // Iterative Fibonacci(10) = 55 using stack frame, loop, jcc, setcc, cmovcc, leave
    #[rustfmt::skip]
    let code = [
        0x55,                         // push ebp
        0x8B, 0xEC,                   // mov ebp, esp
        0x6A, 0x00,                   // push 0
        0x6A, 0x01,                   // push 1
        0xB9, 0x09, 0x00, 0x00, 0x00, // mov ecx, 9
        // loop_head (+12):
        0x8B, 0x45, 0xFC,             // mov eax, [ebp-4] (a)
        0x8B, 0x55, 0xF8,             // mov edx, [ebp-8] (b)
        0x03, 0xC2,                   // add eax, edx
        0x89, 0x55, 0xFC,             // mov [ebp-4], edx
        0x89, 0x45, 0xF8,             // mov [ebp-8], eax
        0x49,                         // dec ecx
        0x7F, 0xEF,                   // jg -17 -> loop_head
        0x83, 0xF8, 0x37,             // cmp eax, 55
        0x0F, 0x94, 0xC3,             // sete bl
        0xBE, 0x99, 0x00, 0x00, 0x00, // mov esi, 0x99
        0x0F, 0x44, 0xFE,             // cmove edi, esi
        0xC9,                         // leave
        0xC3,                         // ret
    ];
    assert_matches_unicorn(DifferentialCase::new("control_flow_fib_and_cmov", &code));

    // Verify ret imm16 pops extra stack bytes identically
    let ret_imm = [0xB8, 0x2A, 0x00, 0x00, 0x00, 0xC2, 0x08, 0x00];
    assert_matches_unicorn(DifferentialCase::new("ret_imm16", &ret_imm));
}

#[test]
fn diff_string_ops() {
    let mut src_dwords = Vec::new();
    for val in [0x1111_2222u32, 0x3333_4444, 0x5555_6666, 0x7777_8888] {
        src_dwords.extend_from_slice(&val.to_le_bytes());
    }

    // 1. cld; rep movsd
    let movsd_code = [0xFC, 0xF3, 0xA5, 0xC3];
    assert_matches_unicorn(
        DifferentialCase::new("rep_movsd_fwd", &movsd_code)
            .with_gpr(Gpr::Esi, DATA_BASE + 0x100)
            .with_gpr(Gpr::Edi, DATA_BASE + 0x200)
            .with_gpr(Gpr::Ecx, 4)
            .with_data(&[(0x100, &src_dwords)]),
    );

    // 2. std; rep movsb; cld
    let movsb_rev = [0xFD, 0xF3, 0xA4, 0xFC, 0xC3];
    assert_matches_unicorn(
        DifferentialCase::new("rep_movsb_rev", &movsb_rev)
            .with_gpr(Gpr::Esi, DATA_BASE + 0x105)
            .with_gpr(Gpr::Edi, DATA_BASE + 0x205)
            .with_gpr(Gpr::Ecx, 6)
            .with_data(&[(0x100, b"ABCDEF")]),
    );

    // 3. rep stosd
    let stosd_code = [0xFC, 0xF3, 0xAB, 0xC3];
    assert_matches_unicorn(
        DifferentialCase::new("rep_stosd", &stosd_code)
            .with_gpr(Gpr::Eax, 0xCAFE_BABE)
            .with_gpr(Gpr::Edi, DATA_BASE + 0x300)
            .with_gpr(Gpr::Ecx, 5),
    );

    // 4. repe cmpsb
    let cmpsb_code = [0xFC, 0xF3, 0xA6, 0xC3];
    assert_matches_unicorn(
        DifferentialCase::new("repe_cmpsb", &cmpsb_code)
            .with_gpr(Gpr::Esi, DATA_BASE + 0x100)
            .with_gpr(Gpr::Edi, DATA_BASE + 0x200)
            .with_gpr(Gpr::Ecx, 6)
            .with_data(&[(0x100, b"TLC_X1"), (0x200, b"TLC_A1")]),
    );

    // 5. repne scasb
    let scasb_code = [0xFC, 0xF2, 0xAE, 0xC3];
    assert_matches_unicorn(
        DifferentialCase::new("repne_scasb", &scasb_code)
            .with_gpr(Gpr::Eax, 0)
            .with_gpr(Gpr::Edi, DATA_BASE + 0x100)
            .with_gpr(Gpr::Ecx, 16)
            .with_data(&[(0x100, b"James\0")]),
    );
}

#[test]
fn diff_x87_fpu() {
    let in_f32_addr = DATA_BASE + 0x100;
    let in_i32_addr = DATA_BASE + 0x104;
    let in_f64_addr = DATA_BASE + 0x108;
    let out_f32_addr = DATA_BASE + 0x200;
    let out_f64_addr = DATA_BASE + 0x208;
    let out_i32_addr = DATA_BASE + 0x210;

    let mut code = Vec::new();
    // fld dword ptr [in_f32_addr] (st0 = 3.5)
    code.extend_from_slice(&[0xD9, 0x05]);
    code.extend_from_slice(&in_f32_addr.to_le_bytes());
    // fild dword ptr [in_i32_addr] (st0 = 6.0, st1 = 3.5)
    code.extend_from_slice(&[0xDB, 0x05]);
    code.extend_from_slice(&in_i32_addr.to_le_bytes());
    // fadd st(0), st(1) -> 9.5
    code.extend_from_slice(&[0xD8, 0xC1]);
    // fmul st(0), st(1) -> 33.25
    code.extend_from_slice(&[0xD8, 0xC9]);
    // fsub st(0), st(1) -> 29.75
    code.extend_from_slice(&[0xD8, 0xE1]);
    // fchs -> -29.75
    code.extend_from_slice(&[0xD9, 0xE0]);
    // fabs -> 29.75
    code.extend_from_slice(&[0xD9, 0xE1]);
    // fdiv dword ptr [in_f32_addr] -> 8.5
    code.extend_from_slice(&[0xD8, 0x35]);
    code.extend_from_slice(&in_f32_addr.to_le_bytes());
    // fld qword ptr [in_f64_addr] (st0 = 16.0, st1 = 8.5, st2 = 3.5)
    code.extend_from_slice(&[0xDD, 0x05]);
    code.extend_from_slice(&in_f64_addr.to_le_bytes());
    // fsqrt -> 4.0
    code.extend_from_slice(&[0xD9, 0xFA]);
    // fcomip st(0), st(1) -> 4.0 < 8.5 (CF=1, ZF=0), pops -> st0 = 8.5
    code.extend_from_slice(&[0xDF, 0xF1]);
    // setb bl
    code.extend_from_slice(&[0x0F, 0x92, 0xC3]);
    // fst dword ptr [out_f32_addr] (8.5f32)
    code.extend_from_slice(&[0xD9, 0x15]);
    code.extend_from_slice(&out_f32_addr.to_le_bytes());
    // fistp dword ptr [out_i32_addr] (rounds 8.5 -> 8 nearest-even!)
    code.extend_from_slice(&[0xDB, 0x1D]);
    code.extend_from_slice(&out_i32_addr.to_le_bytes());
    // fstp qword ptr [out_f64_addr] (3.5f64)
    code.extend_from_slice(&[0xDD, 0x1D]);
    code.extend_from_slice(&out_f64_addr.to_le_bytes());
    code.push(0xC3);

    assert_matches_unicorn(
        DifferentialCase::new("x87_fpu_pipeline", &code)
            .with_data(&[
                (0x100, &3.5f32.to_le_bytes()),
                (0x104, &6i32.to_le_bytes()),
                (0x108, &16.0f64.to_le_bytes()),
            ])
            .with_flags_mask(EFLAGS_CF | EFLAGS_PF | EFLAGS_ZF),
    );
}

#[test]
fn diff_sse_simd() {
    let vec_a_addr = DATA_BASE + 0x100;
    let vec_b_addr = DATA_BASE + 0x110;
    let out_vec_addr = DATA_BASE + 0x200;

    let mut vec_a = Vec::new();
    for v in [1.0f32, 2.0, 3.0, 4.0] {
        vec_a.extend_from_slice(&v.to_le_bytes());
    }
    let mut vec_b = Vec::new();
    for v in [5.0f32, 4.0, 3.0, 2.0] {
        vec_b.extend_from_slice(&v.to_le_bytes());
    }

    let mut code = Vec::new();
    // movups xmm0, [vec_a_addr]
    code.extend_from_slice(&[0x0F, 0x10, 0x05]);
    code.extend_from_slice(&vec_a_addr.to_le_bytes());
    // movups xmm1, [vec_b_addr]
    code.extend_from_slice(&[0x0F, 0x10, 0x0D]);
    code.extend_from_slice(&vec_b_addr.to_le_bytes());
    // addps xmm0, xmm1 -> [6, 6, 6, 6]
    code.extend_from_slice(&[0x0F, 0x58, 0xC1]);
    // subps xmm0, xmm1 -> [1, 2, 3, 4]
    code.extend_from_slice(&[0x0F, 0x5C, 0xC1]);
    // mulps xmm0, xmm1 -> [5, 8, 9, 8]
    code.extend_from_slice(&[0x0F, 0x59, 0xC1]);
    // shufps xmm0, xmm1, 0x4E -> [9, 8, 5, 4]
    code.extend_from_slice(&[0x0F, 0xC6, 0xC1, 0x4E]);
    // divps xmm0, [vec_a_addr] -> [9/1, 8/2, 5/3, 4/4] = [9.0, 4.0, 5/3, 1.0]
    code.extend_from_slice(&[0x0F, 0x5E, 0x05]);
    code.extend_from_slice(&vec_a_addr.to_le_bytes());
    // movaps xmm2, xmm0
    code.extend_from_slice(&[0x0F, 0x28, 0xD0]);
    // xorps xmm3, xmm3
    code.extend_from_slice(&[0x0F, 0x57, 0xDB]);
    // cvtsi2ss xmm3, ecx (ecx = 7 -> 7.0)
    code.extend_from_slice(&[0xF3, 0x0F, 0x2A, 0xD9]);
    // addss xmm2, xmm3 (xmm2[0] = 9.0 + 7.0 = 16.0)
    code.extend_from_slice(&[0xF3, 0x0F, 0x58, 0xD3]);
    // mulss xmm2, xmm3 (xmm2[0] = 16.0 * 7.0 = 112.0)
    code.extend_from_slice(&[0xF3, 0x0F, 0x59, 0xD3]);
    // cvttss2si eax, xmm2 -> eax = 112
    code.extend_from_slice(&[0xF3, 0x0F, 0x2C, 0xC2]);
    // ucomiss xmm2, xmm3 -> 112.0 > 7.0 (CF=0, ZF=0, PF=0)
    code.extend_from_slice(&[0x0F, 0x2E, 0xD3]);
    // movups [out_vec_addr], xmm2
    code.extend_from_slice(&[0x0F, 0x11, 0x15]);
    code.extend_from_slice(&out_vec_addr.to_le_bytes());
    code.push(0xC3);

    assert_matches_unicorn(
        DifferentialCase::new("sse_simd_pipeline", &code)
            .with_gpr(Gpr::Ecx, 7)
            .with_data(&[(0x100, &vec_a), (0x110, &vec_b)])
            .with_flags_mask(EFLAGS_CF | EFLAGS_PF | EFLAGS_ZF)
            .with_xmm_check(4),
    );
}
