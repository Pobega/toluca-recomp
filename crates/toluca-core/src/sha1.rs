const SHA1_BLOCK_BYTES: usize = 64;
const SHA1_LENGTH_BYTES: usize = 8;
const SHA1_DIGEST_BYTES: usize = 20;
const SHA1_ROUNDS: usize = 80;

const SHA1_INITIAL_STATE: [u32; 5] = [
    0x6745_2301,
    0xEFCD_AB89,
    0x98BA_DCFE,
    0x1032_5476,
    0xC3D2_E1F0,
];

const SHA1_K0: u32 = 0x5A82_7999;
const SHA1_K1: u32 = 0x6ED9_EBA1;
const SHA1_K2: u32 = 0x8F1B_BCDC;
const SHA1_K3: u32 = 0xCA62_C1D6;

pub fn sha1(data: &[u8]) -> [u8; SHA1_DIGEST_BYTES] {
    let mut state = SHA1_INITIAL_STATE;

    let mut chunks = data.chunks_exact(SHA1_BLOCK_BYTES);
    for block in &mut chunks {
        compress_block(&mut state, block);
    }

    let remainder = chunks.remainder();
    let mut tail = [0u8; SHA1_BLOCK_BYTES * 2];
    tail[..remainder.len()].copy_from_slice(remainder);
    tail[remainder.len()] = 0x80;

    let total_blocks = if remainder.len() + 1 + SHA1_LENGTH_BYTES > SHA1_BLOCK_BYTES {
        2
    } else {
        1
    };
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let len_offset = total_blocks * SHA1_BLOCK_BYTES - SHA1_LENGTH_BYTES;
    tail[len_offset..len_offset + SHA1_LENGTH_BYTES].copy_from_slice(&bit_len.to_be_bytes());

    for block_idx in 0..total_blocks {
        let start = block_idx * SHA1_BLOCK_BYTES;
        compress_block(&mut state, &tail[start..start + SHA1_BLOCK_BYTES]);
    }

    let mut digest = [0u8; SHA1_DIGEST_BYTES];
    for (idx, word) in state.iter().enumerate() {
        let start = idx * 4;
        digest[start..start + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

pub fn format_hex(digest: &[u8; SHA1_DIGEST_BYTES]) -> String {
    let mut out = String::with_capacity(SHA1_DIGEST_BYTES * 2);
    for &byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn compress_block(state: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; SHA1_ROUNDS];
    for (idx, slot) in w.iter_mut().take(16).enumerate() {
        let off = idx * 4;
        *slot = u32::from_be_bytes([block[off], block[off + 1], block[off + 2], block[off + 3]]);
    }
    for idx in 16..SHA1_ROUNDS {
        w[idx] = (w[idx - 3] ^ w[idx - 8] ^ w[idx - 14] ^ w[idx - 16]).rotate_left(1);
    }

    let [mut a, mut b, mut c, mut d, mut e] = *state;

    for (round, &word) in w.iter().enumerate() {
        let (f, k) = match round {
            0..=19 => ((b & c) | ((!b) & d), SHA1_K0),
            20..=39 => (b ^ c ^ d, SHA1_K1),
            40..=59 => ((b & c) | (b & d) | (c & d), SHA1_K2),
            _ => (b ^ c ^ d, SHA1_K3),
        };

        let temp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = temp;
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3174_test_vectors() {
        assert_eq!(
            format_hex(&sha1(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            format_hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            format_hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }
}
