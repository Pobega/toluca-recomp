use std::collections::{BTreeMap, BTreeSet, HashMap};

use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind};
use toluca_core::GuestAddr;

use crate::cfg::DiscoveredFunction;
use crate::pe::PeImage;

const SHIFT_COUNT_MASK: u8 = 0x1F;

pub struct FunctionFlagLiveness {
    arith_live_in: HashMap<GuestAddr, bool>,
    cf_live_in: HashMap<GuestAddr, bool>,
}

impl FunctionFlagLiveness {
    pub fn empty() -> Self {
        Self {
            arith_live_in: HashMap::new(),
            cf_live_in: HashMap::new(),
        }
    }

    pub fn is_arith_live_in(&self, va: GuestAddr) -> bool {
        self.arith_live_in.get(&va).copied().unwrap_or(true)
    }

    pub fn is_cf_live_in(&self, va: GuestAddr) -> bool {
        self.cf_live_in.get(&va).copied().unwrap_or(true)
    }

    pub fn block_live_out(
        &self,
        instrs: &[Instruction],
        has_invalid: bool,
        fallthrough_va: GuestAddr,
    ) -> (bool, bool) {
        block_live_out_with_maps(
            &self.arith_live_in,
            &self.cf_live_in,
            instrs,
            has_invalid,
            fallthrough_va,
        )
    }
}

pub struct DecodedSplitBlock {
    pub start_va: GuestAddr,
    pub instrs: Vec<Instruction>,
    pub has_invalid: bool,
    pub end_va: GuestAddr,
}

pub fn decode_split_block(
    image: &PeImage,
    func: &DiscoveredFunction,
    split_list: &[GuestAddr],
    idx: usize,
) -> Option<DecodedSplitBlock> {
    let start_va = split_list[idx];
    let containing_end = containing_block_end(func, start_va)?;
    let next_split = split_list.get(idx + 1).copied();
    let limit_va = next_split.map_or(containing_end, |s| s.min(containing_end));
    let byte_len = limit_va.saturating_sub(start_va) as usize;
    let bytes = image.read_bytes_at_va(start_va, byte_len)?;

    let mut decoder = Decoder::with_ip(32, bytes, start_va as u64, DecoderOptions::NONE);
    let mut instrs = Vec::new();
    let mut has_invalid = false;
    let mut current_end = start_va;

    while decoder.can_decode() {
        let mut instr = Instruction::default();
        decoder.decode_out(&mut instr);
        if instr.is_invalid() {
            has_invalid = true;
            instrs.push(instr);
            break;
        }
        current_end = (instr.ip() as GuestAddr).saturating_add(instr.len() as u32);
        let terminates = is_block_terminator(&instr);
        instrs.push(instr);
        if terminates {
            break;
        }
    }

    Some(DecodedSplitBlock {
        start_va,
        instrs,
        has_invalid,
        end_va: current_end,
    })
}

pub fn compute_function_flag_liveness(
    image: &PeImage,
    func: &DiscoveredFunction,
    split_vas: &BTreeSet<GuestAddr>,
) -> FunctionFlagLiveness {
    let split_list: Vec<GuestAddr> = split_vas.iter().copied().collect();
    let mut arith_live_in: HashMap<GuestAddr, bool> =
        split_list.iter().map(|&va| (va, false)).collect();
    let mut cf_live_in: HashMap<GuestAddr, bool> =
        split_list.iter().map(|&va| (va, false)).collect();

    let mut decoded_blocks: BTreeMap<GuestAddr, Option<DecodedSplitBlock>> = BTreeMap::new();
    for idx in 0..split_list.len() {
        let va = split_list[idx];
        decoded_blocks.insert(va, decode_split_block(image, func, &split_list, idx));
    }

    loop {
        let mut changed = false;
        for &va in split_list.iter().rev() {
            let (mut a, mut c) = match &decoded_blocks[&va] {
                Some(block) => block_live_out_with_maps(
                    &arith_live_in,
                    &cf_live_in,
                    &block.instrs,
                    block.has_invalid,
                    block.end_va,
                ),
                None => (true, true),
            };
            if let Some(block) = &decoded_blocks[&va] {
                for instr in block.instrs.iter().rev() {
                    if !instr.is_invalid() {
                        transfer_instr_backward(instr, &mut a, &mut c);
                    }
                }
            }
            if arith_live_in.get(&va).copied() != Some(a) {
                arith_live_in.insert(va, a);
                changed = true;
            }
            if cf_live_in.get(&va).copied() != Some(c) {
                cf_live_in.insert(va, c);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    FunctionFlagLiveness {
        arith_live_in,
        cf_live_in,
    }
}

pub fn compute_block_instr_liveness(
    instrs: &[Instruction],
    arith_live_out: bool,
    cf_live_out: bool,
) -> (Vec<bool>, Vec<bool>) {
    let n = instrs.len();
    let mut arith_after = vec![false; n];
    let mut cf_after = vec![false; n];
    let mut a = arith_live_out;
    let mut c = cf_live_out;
    for i in (0..n).rev() {
        arith_after[i] = a;
        cf_after[i] = c;
        if !instrs[i].is_invalid() {
            transfer_instr_backward(&instrs[i], &mut a, &mut c);
        }
    }
    (arith_after, cf_after)
}

fn block_live_out_with_maps(
    arith_live_in: &HashMap<GuestAddr, bool>,
    cf_live_in: &HashMap<GuestAddr, bool>,
    instrs: &[Instruction],
    has_invalid: bool,
    fallthrough_va: GuestAddr,
) -> (bool, bool) {
    if has_invalid {
        return (true, true);
    }
    let Some(last) = instrs.last() else {
        return (true, true);
    };
    if matches!(
        last.mnemonic(),
        Mnemonic::Hlt | Mnemonic::Int3 | Mnemonic::Ud2
    ) {
        return (true, true);
    }
    if matches!(
        last.mnemonic(),
        Mnemonic::Jecxz | Mnemonic::Jcxz | Mnemonic::Loop | Mnemonic::Loope | Mnemonic::Loopne
    ) {
        let taken = last.near_branch32();
        return union_two_targets(arith_live_in, cf_live_in, taken, fallthrough_va);
    }
    match last.flow_control() {
        FlowControl::UnconditionalBranch => {
            let target = last.near_branch32();
            lookup_target(arith_live_in, cf_live_in, target)
        }
        FlowControl::ConditionalBranch => {
            let taken = last.near_branch32();
            union_two_targets(arith_live_in, cf_live_in, taken, fallthrough_va)
        }
        FlowControl::IndirectBranch | FlowControl::Return => (true, true),
        _ => lookup_target(arith_live_in, cf_live_in, fallthrough_va),
    }
}

fn lookup_target(
    arith_live_in: &HashMap<GuestAddr, bool>,
    cf_live_in: &HashMap<GuestAddr, bool>,
    target: GuestAddr,
) -> (bool, bool) {
    (
        arith_live_in.get(&target).copied().unwrap_or(true),
        cf_live_in.get(&target).copied().unwrap_or(true),
    )
}

fn union_two_targets(
    arith_live_in: &HashMap<GuestAddr, bool>,
    cf_live_in: &HashMap<GuestAddr, bool>,
    t0: GuestAddr,
    t1: GuestAddr,
) -> (bool, bool) {
    let (a0, c0) = lookup_target(arith_live_in, cf_live_in, t0);
    let (a1, c1) = lookup_target(arith_live_in, cf_live_in, t1);
    (a0 || a1, c0 || c1)
}

fn transfer_instr_backward(instr: &Instruction, arith_live: &mut bool, cf_live: &mut bool) {
    let mnemonic = instr.mnemonic();

    // 1. Kill flags unconditionally overwritten without reading prior flags.
    if kills_all_arith_flags(instr, mnemonic) {
        *arith_live = false;
        *cf_live = false;
    } else if matches!(mnemonic, Mnemonic::Clc | Mnemonic::Stc | Mnemonic::Sahf) {
        *cf_live = false;
    }

    // 2. Mark flags read by this instruction.
    if reads_cf_flag(instr, mnemonic) {
        *arith_live = true;
        *cf_live = true;
    } else if reads_non_cf_arith_flags(mnemonic) {
        *arith_live = true;
    }
}

fn kills_all_arith_flags(instr: &Instruction, mnemonic: Mnemonic) -> bool {
    if matches!(
        mnemonic,
        Mnemonic::Add
            | Mnemonic::Sub
            | Mnemonic::Cmp
            | Mnemonic::Test
            | Mnemonic::And
            | Mnemonic::Or
            | Mnemonic::Xor
            | Mnemonic::Neg
            | Mnemonic::Popf
            | Mnemonic::Popfd
            | Mnemonic::Fcomi
            | Mnemonic::Fucomi
            | Mnemonic::Fcomip
            | Mnemonic::Fucomip
            | Mnemonic::Comiss
            | Mnemonic::Ucomiss
            | Mnemonic::Comisd
            | Mnemonic::Ucomisd
    ) {
        return true;
    }
    if matches!(
        mnemonic,
        Mnemonic::Shl | Mnemonic::Sal | Mnemonic::Shr | Mnemonic::Sar
    ) && instr.op_count() >= 2
    {
        let imm = match instr.op_kind(1) {
            OpKind::Immediate8 => Some(instr.immediate8()),
            OpKind::Immediate8_2nd => Some(instr.immediate8_2nd()),
            _ => None,
        };
        return imm.is_some_and(|v| (v & SHIFT_COUNT_MASK) != 0);
    }
    false
}

fn reads_cf_flag(instr: &Instruction, mnemonic: Mnemonic) -> bool {
    if matches!(
        instr.flow_control(),
        FlowControl::Return
            | FlowControl::Call
            | FlowControl::IndirectCall
            | FlowControl::IndirectBranch
    ) {
        return true;
    }
    matches!(
        mnemonic,
        Mnemonic::Jb
            | Mnemonic::Jae
            | Mnemonic::Jbe
            | Mnemonic::Ja
            | Mnemonic::Setb
            | Mnemonic::Setae
            | Mnemonic::Setbe
            | Mnemonic::Seta
            | Mnemonic::Cmovb
            | Mnemonic::Cmovae
            | Mnemonic::Cmovbe
            | Mnemonic::Cmova
            | Mnemonic::Fcmovb
            | Mnemonic::Fcmovnb
            | Mnemonic::Fcmovbe
            | Mnemonic::Fcmovnbe
            | Mnemonic::Adc
            | Mnemonic::Sbb
            | Mnemonic::Rcl
            | Mnemonic::Rcr
            | Mnemonic::Cmc
            | Mnemonic::Pushf
            | Mnemonic::Pushfd
            | Mnemonic::Lahf
            | Mnemonic::Daa
            | Mnemonic::Das
            | Mnemonic::Aaa
            | Mnemonic::Aas
            | Mnemonic::Into
            | Mnemonic::Hlt
            | Mnemonic::Int3
            | Mnemonic::Ud2
            | Mnemonic::Cpuid
    )
}

fn reads_non_cf_arith_flags(mnemonic: Mnemonic) -> bool {
    matches!(
        mnemonic,
        Mnemonic::Je
            | Mnemonic::Jne
            | Mnemonic::Jl
            | Mnemonic::Jge
            | Mnemonic::Jle
            | Mnemonic::Jg
            | Mnemonic::Js
            | Mnemonic::Jns
            | Mnemonic::Jo
            | Mnemonic::Jno
            | Mnemonic::Jp
            | Mnemonic::Jnp
            | Mnemonic::Sete
            | Mnemonic::Setne
            | Mnemonic::Setl
            | Mnemonic::Setge
            | Mnemonic::Setle
            | Mnemonic::Setg
            | Mnemonic::Sets
            | Mnemonic::Setns
            | Mnemonic::Seto
            | Mnemonic::Setno
            | Mnemonic::Setp
            | Mnemonic::Setnp
            | Mnemonic::Cmove
            | Mnemonic::Cmovne
            | Mnemonic::Cmovl
            | Mnemonic::Cmovge
            | Mnemonic::Cmovle
            | Mnemonic::Cmovg
            | Mnemonic::Cmovs
            | Mnemonic::Cmovns
            | Mnemonic::Cmovo
            | Mnemonic::Cmovno
            | Mnemonic::Cmovp
            | Mnemonic::Cmovnp
            | Mnemonic::Fcmove
            | Mnemonic::Fcmovne
            | Mnemonic::Fcmovu
            | Mnemonic::Fcmovnu
            | Mnemonic::Loope
            | Mnemonic::Loopne
    )
}

fn is_block_terminator(instr: &Instruction) -> bool {
    matches!(
        instr.flow_control(),
        FlowControl::UnconditionalBranch
            | FlowControl::ConditionalBranch
            | FlowControl::IndirectBranch
            | FlowControl::Return
    ) || matches!(
        instr.mnemonic(),
        Mnemonic::Hlt
            | Mnemonic::Int3
            | Mnemonic::Ud2
            | Mnemonic::Jecxz
            | Mnemonic::Jcxz
            | Mnemonic::Loop
            | Mnemonic::Loope
            | Mnemonic::Loopne
    )
}

fn containing_block_end(func: &DiscoveredFunction, va: GuestAddr) -> Option<GuestAddr> {
    func.blocks
        .values()
        .find(|block| va >= block.start_va && va < block.end_va)
        .map(|block| block.end_va)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(base_va: GuestAddr, code: &[u8]) -> Vec<Instruction> {
        let mut dec = Decoder::with_ip(32, code, base_va as u64, DecoderOptions::NONE);
        let mut out = Vec::new();
        while dec.can_decode() {
            let mut instr = Instruction::default();
            dec.decode_out(&mut instr);
            out.push(instr);
        }
        out
    }

    #[test]
    fn block_liveness_kills_dead_flags_and_preserves_inc_cf_dependency() {
        // add eax, 1; cmp eax, 5; jg +2; xor eax, eax
        #[rustfmt::skip]
        let code = [
            0x83, 0xC0, 0x01, // add eax, 1 (dead: killed by cmp)
            0x83, 0xF8, 0x05, // cmp eax, 5 (live for jg)
            0x7F, 0x02,       // jg +2
            0x31, 0xC0,       // xor eax, eax
        ];
        let instrs = decode_all(0x0040_1000, &code);
        let (arith, cf) = compute_block_instr_liveness(&instrs, false, false);
        assert_eq!(arith, vec![false, true, false, false]);
        assert_eq!(cf, vec![false, false, false, false]);

        // stc; inc eax; jb +2 -> cf_live after stc & inc must both be true
        #[rustfmt::skip]
        let cf_code = [
            0xF9,             // stc
            0x40,             // inc eax
            0x72, 0x02,       // jb +2
        ];
        let cf_instrs = decode_all(0x0040_1000, &cf_code);
        let (a_cf, c_cf) = compute_block_instr_liveness(&cf_instrs, false, false);
        assert_eq!(a_cf, vec![true, true, false]);
        assert_eq!(c_cf, vec![true, true, false]);

        let empty = FunctionFlagLiveness::empty();
        assert!(empty.is_arith_live_in(0x0040_2000));
        assert!(empty.is_cf_live_in(0x0040_2000));
    }
}
