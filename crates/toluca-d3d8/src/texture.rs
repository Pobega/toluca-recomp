use crate::types::{
    D3DFMT_A1R5G5B5, D3DFMT_A4R4G4B4, D3DFMT_A8, D3DFMT_A8L8, D3DFMT_A8R8G8B8, D3DFMT_DXT1,
    D3DFMT_DXT2, D3DFMT_DXT3, D3DFMT_DXT4, D3DFMT_DXT5, D3DFMT_L8, D3DFMT_P8, D3DFMT_R5G6B5,
    D3DFMT_R8G8B8, D3DFMT_V8U8, D3DFMT_X1R5G5B5, D3DFMT_X4R4G4B4, D3DFMT_X8R8G8B8,
    format_bytes_per_pixel, surface_row_pitch,
};

const DXT_BLOCK_DIM: u32 = 4;
const DXT1_BLOCK_BYTES: usize = 8;
const DXT3_DXT5_BLOCK_BYTES: usize = 16;
const RGBA8_BPP: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedRgba8 {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreparedTextureUpload {
    Rgba8(DecodedRgba8),
    CompressedS3tc {
        format: u32,
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
}

fn expand_5bit(v: u8) -> u8 {
    (v << 3) | (v >> 2)
}

fn expand_6bit(v: u8) -> u8 {
    (v << 2) | (v >> 4)
}

fn rgb565_to_rgb8(color: u16) -> [u8; 3] {
    let r5 = ((color >> 11) & 0x1F) as u8;
    let g6 = ((color >> 5) & 0x3F) as u8;
    let b5 = (color & 0x1F) as u8;
    [expand_5bit(r5), expand_6bit(g6), expand_5bit(b5)]
}

fn dxt_color_palette(c0: u16, c1: u16, allow_transparent: bool) -> [[u8; 4]; 4] {
    let rgb0 = rgb565_to_rgb8(c0);
    let rgb1 = rgb565_to_rgb8(c1);
    let p0 = [rgb0[0], rgb0[1], rgb0[2], 255];
    let p1 = [rgb1[0], rgb1[1], rgb1[2], 255];

    if !allow_transparent || c0 > c1 {
        let p2 = [
            ((2 * u16::from(rgb0[0]) + u16::from(rgb1[0])) / 3) as u8,
            ((2 * u16::from(rgb0[1]) + u16::from(rgb1[1])) / 3) as u8,
            ((2 * u16::from(rgb0[2]) + u16::from(rgb1[2])) / 3) as u8,
            255,
        ];
        let p3 = [
            ((u16::from(rgb0[0]) + 2 * u16::from(rgb1[0])) / 3) as u8,
            ((u16::from(rgb0[1]) + 2 * u16::from(rgb1[1])) / 3) as u8,
            ((u16::from(rgb0[2]) + 2 * u16::from(rgb1[2])) / 3) as u8,
            255,
        ];
        [p0, p1, p2, p3]
    } else {
        let p2 = [
            ((u16::from(rgb0[0]) + u16::from(rgb1[0])) / 2) as u8,
            ((u16::from(rgb0[1]) + u16::from(rgb1[1])) / 2) as u8,
            ((u16::from(rgb0[2]) + u16::from(rgb1[2])) / 2) as u8,
            255,
        ];
        [p0, p1, p2, [0, 0, 0, 0]]
    }
}

fn unpremultiply_channel(c: u8, alpha: u8) -> u8 {
    if alpha == 0 || alpha == 255 {
        return c;
    }
    ((u16::from(c) * 255) / u16::from(alpha)).min(255) as u8
}

fn write_rgba8_pixel(out: &mut [u8], width: u32, x: u32, y: u32, rgba: [u8; 4]) {
    let idx = ((y as usize) * (width as usize) + (x as usize)) * RGBA8_BPP;
    out[idx..idx + RGBA8_BPP].copy_from_slice(&rgba);
}

fn prepare_dxt_pixels(
    mut pixels: Vec<u8>,
    needed: usize,
    src_len: usize,
    blocks_w: u32,
    blocks_h: u32,
    block_bytes: usize,
) -> Vec<u8> {
    let prev_len = pixels.len();
    pixels.resize(needed, 0);
    let expected = (blocks_w as usize) * (blocks_h as usize) * block_bytes;
    if src_len < expected && prev_len > 0 {
        pixels[..prev_len.min(needed)].fill(0);
    }
    pixels
}

fn decode_dxt1_to_rgba8_into(
    src: &[u8],
    width: u32,
    height: u32,
    mut pixels: Vec<u8>,
) -> DecodedRgba8 {
    let needed = (width as usize) * (height as usize) * RGBA8_BPP;
    if width == 0 || height == 0 {
        pixels.clear();
        return DecodedRgba8 {
            width,
            height,
            pixels,
        };
    }

    let blocks_w = width.div_ceil(DXT_BLOCK_DIM);
    let blocks_h = height.div_ceil(DXT_BLOCK_DIM);
    pixels = prepare_dxt_pixels(pixels, needed, src.len(), blocks_w, blocks_h, DXT1_BLOCK_BYTES);

    for by in 0..blocks_h {
        for bx in 0..blocks_w {
            let block_idx = ((by * blocks_w + bx) as usize) * DXT1_BLOCK_BYTES;
            if block_idx + DXT1_BLOCK_BYTES > src.len() {
                break;
            }
            let block = &src[block_idx..block_idx + DXT1_BLOCK_BYTES];
            let c0 = u16::from_le_bytes([block[0], block[1]]);
            let c1 = u16::from_le_bytes([block[2], block[3]]);
            let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
            let palette = dxt_color_palette(c0, c1, true);

            for py in 0..DXT_BLOCK_DIM {
                let gy = by * DXT_BLOCK_DIM + py;
                if gy >= height {
                    continue;
                }
                for px in 0..DXT_BLOCK_DIM {
                    let gx = bx * DXT_BLOCK_DIM + px;
                    if gx >= width {
                        continue;
                    }
                    let texel = py * DXT_BLOCK_DIM + px;
                    let color_idx = ((indices >> (texel * 2)) & 0x3) as usize;
                    write_rgba8_pixel(&mut pixels, width, gx, gy, palette[color_idx]);
                }
            }
        }
    }

    DecodedRgba8 {
        width,
        height,
        pixels,
    }
}

pub fn decode_dxt1_to_rgba8(src: &[u8], width: u32, height: u32) -> DecodedRgba8 {
    decode_dxt1_to_rgba8_into(src, width, height, Vec::new())
}

fn decode_dxt3_internal(
    src: &[u8],
    width: u32,
    height: u32,
    premultiplied: bool,
    mut pixels: Vec<u8>,
) -> DecodedRgba8 {
    let needed = (width as usize) * (height as usize) * RGBA8_BPP;
    if width == 0 || height == 0 {
        pixels.clear();
        return DecodedRgba8 {
            width,
            height,
            pixels,
        };
    }

    let blocks_w = width.div_ceil(DXT_BLOCK_DIM);
    let blocks_h = height.div_ceil(DXT_BLOCK_DIM);
    pixels = prepare_dxt_pixels(
        pixels,
        needed,
        src.len(),
        blocks_w,
        blocks_h,
        DXT3_DXT5_BLOCK_BYTES,
    );

    for by in 0..blocks_h {
        for bx in 0..blocks_w {
            let block_idx = ((by * blocks_w + bx) as usize) * DXT3_DXT5_BLOCK_BYTES;
            if block_idx + DXT3_DXT5_BLOCK_BYTES > src.len() {
                break;
            }
            let block = &src[block_idx..block_idx + DXT3_DXT5_BLOCK_BYTES];
            let alpha_bits = u64::from_le_bytes(block[0..8].try_into().unwrap());
            let c0 = u16::from_le_bytes([block[8], block[9]]);
            let c1 = u16::from_le_bytes([block[10], block[11]]);
            let indices = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);
            let palette = dxt_color_palette(c0, c1, false);

            for py in 0..DXT_BLOCK_DIM {
                let gy = by * DXT_BLOCK_DIM + py;
                if gy >= height {
                    continue;
                }
                for px in 0..DXT_BLOCK_DIM {
                    let gx = bx * DXT_BLOCK_DIM + px;
                    if gx >= width {
                        continue;
                    }
                    let texel = py * DXT_BLOCK_DIM + px;
                    let a4 = ((alpha_bits >> (texel * 4)) & 0xF) as u8;
                    let alpha = a4 * 17;
                    let mut rgba = palette[((indices >> (texel * 2)) & 0x3) as usize];
                    if premultiplied {
                        rgba[0] = unpremultiply_channel(rgba[0], alpha);
                        rgba[1] = unpremultiply_channel(rgba[1], alpha);
                        rgba[2] = unpremultiply_channel(rgba[2], alpha);
                    }
                    rgba[3] = alpha;
                    write_rgba8_pixel(&mut pixels, width, gx, gy, rgba);
                }
            }
        }
    }

    DecodedRgba8 {
        width,
        height,
        pixels,
    }
}

pub fn decode_dxt3_to_rgba8(src: &[u8], width: u32, height: u32) -> DecodedRgba8 {
    decode_dxt3_internal(src, width, height, false, Vec::new())
}

fn dxt5_alpha_table(a0: u8, a1: u8) -> [u8; 8] {
    let mut table = [0u8; 8];
    table[0] = a0;
    table[1] = a1;
    if a0 > a1 {
        for i in 1..=6 {
            let num = (7 - i) as u16 * u16::from(a0) + (i as u16) * u16::from(a1);
            table[i + 1] = (num / 7) as u8;
        }
    } else {
        for i in 1..=4 {
            let num = (5 - i) as u16 * u16::from(a0) + (i as u16) * u16::from(a1);
            table[i + 1] = (num / 5) as u8;
        }
        table[6] = 0;
        table[7] = 255;
    }
    table
}

fn decode_dxt5_internal(
    src: &[u8],
    width: u32,
    height: u32,
    premultiplied: bool,
    mut pixels: Vec<u8>,
) -> DecodedRgba8 {
    let needed = (width as usize) * (height as usize) * RGBA8_BPP;
    if width == 0 || height == 0 {
        pixels.clear();
        return DecodedRgba8 {
            width,
            height,
            pixels,
        };
    }

    let blocks_w = width.div_ceil(DXT_BLOCK_DIM);
    let blocks_h = height.div_ceil(DXT_BLOCK_DIM);
    pixels = prepare_dxt_pixels(
        pixels,
        needed,
        src.len(),
        blocks_w,
        blocks_h,
        DXT3_DXT5_BLOCK_BYTES,
    );

    for by in 0..blocks_h {
        for bx in 0..blocks_w {
            let block_idx = ((by * blocks_w + bx) as usize) * DXT3_DXT5_BLOCK_BYTES;
            if block_idx + DXT3_DXT5_BLOCK_BYTES > src.len() {
                break;
            }
            let block = &src[block_idx..block_idx + DXT3_DXT5_BLOCK_BYTES];
            let alpha_table = dxt5_alpha_table(block[0], block[1]);
            let mut alpha_bits = 0u64;
            for i in 0..6 {
                alpha_bits |= u64::from(block[2 + i]) << (i * 8);
            }

            let c0 = u16::from_le_bytes([block[8], block[9]]);
            let c1 = u16::from_le_bytes([block[10], block[11]]);
            let indices = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);
            let palette = dxt_color_palette(c0, c1, false);

            for py in 0..DXT_BLOCK_DIM {
                let gy = by * DXT_BLOCK_DIM + py;
                if gy >= height {
                    continue;
                }
                for px in 0..DXT_BLOCK_DIM {
                    let gx = bx * DXT_BLOCK_DIM + px;
                    if gx >= width {
                        continue;
                    }
                    let texel = py * DXT_BLOCK_DIM + px;
                    let a_idx = ((alpha_bits >> (texel * 3)) & 0x7) as usize;
                    let alpha = alpha_table[a_idx];
                    let mut rgba = palette[((indices >> (texel * 2)) & 0x3) as usize];
                    if premultiplied {
                        rgba[0] = unpremultiply_channel(rgba[0], alpha);
                        rgba[1] = unpremultiply_channel(rgba[1], alpha);
                        rgba[2] = unpremultiply_channel(rgba[2], alpha);
                    }
                    rgba[3] = alpha;
                    write_rgba8_pixel(&mut pixels, width, gx, gy, rgba);
                }
            }
        }
    }

    DecodedRgba8 {
        width,
        height,
        pixels,
    }
}

pub fn decode_dxt5_to_rgba8(src: &[u8], width: u32, height: u32) -> DecodedRgba8 {
    decode_dxt5_internal(src, width, height, false, Vec::new())
}

fn palette_has_nonzero_alpha(palette: &[[u8; 4]; 256]) -> bool {
    palette.iter().any(|entry| entry[3] != 0)
}

fn convert_uncompressed_pixel(
    format: u32,
    row: &[u8],
    x: usize,
    palette: Option<&[[u8; 4]; 256]>,
    pal_uses_alpha: bool,
) -> Result<[u8; 4], String> {
    match format {
        D3DFMT_A8R8G8B8 => {
            let o = x * 4;
            Ok([row[o + 2], row[o + 1], row[o], row[o + 3]])
        }
        D3DFMT_X8R8G8B8 => {
            let o = x * 4;
            Ok([row[o + 2], row[o + 1], row[o], 255])
        }
        D3DFMT_R8G8B8 => {
            let o = x * 3;
            Ok([row[o + 2], row[o + 1], row[o], 255])
        }
        D3DFMT_R5G6B5 => {
            let o = x * 2;
            let val = u16::from_le_bytes([row[o], row[o + 1]]);
            let rgb = rgb565_to_rgb8(val);
            Ok([rgb[0], rgb[1], rgb[2], 255])
        }
        D3DFMT_A1R5G5B5 | D3DFMT_X1R5G5B5 => {
            let o = x * 2;
            let val = u16::from_le_bytes([row[o], row[o + 1]]);
            let r = expand_5bit(((val >> 10) & 0x1F) as u8);
            let g = expand_5bit(((val >> 5) & 0x1F) as u8);
            let b = expand_5bit((val & 0x1F) as u8);
            let a = if format == D3DFMT_A1R5G5B5 && (val & 0x8000) == 0 {
                0
            } else {
                255
            };
            Ok([r, g, b, a])
        }
        D3DFMT_A4R4G4B4 | D3DFMT_X4R4G4B4 => {
            let o = x * 2;
            let val = u16::from_le_bytes([row[o], row[o + 1]]);
            let r = (((val >> 8) & 0xF) as u8) * 17;
            let g = (((val >> 4) & 0xF) as u8) * 17;
            let b = ((val & 0xF) as u8) * 17;
            let a = if format == D3DFMT_A4R4G4B4 {
                (((val >> 12) & 0xF) as u8) * 17
            } else {
                255
            };
            Ok([r, g, b, a])
        }
        D3DFMT_L8 => {
            let lum = row[x];
            Ok([lum, lum, lum, 255])
        }
        D3DFMT_A8L8 => {
            let o = x * 2;
            let lum = row[o];
            let alpha = row[o + 1];
            Ok([lum, lum, lum, alpha])
        }
        D3DFMT_A8 => Ok([255, 255, 255, row[x]]),
        D3DFMT_P8 => {
            let idx = row[x] as usize;
            if let Some(pal) = palette {
                let entry = pal[idx];
                let alpha = if pal_uses_alpha { entry[3] } else { 255 };
                Ok([entry[0], entry[1], entry[2], alpha])
            } else {
                let v = idx as u8;
                Ok([v, v, v, 255])
            }
        }
        D3DFMT_V8U8 => {
            let o = x * 2;
            let u = (row[o] as i8).wrapping_add(-128) as u8;
            let v = (row[o + 1] as i8).wrapping_add(-128) as u8;
            Ok([u, v, 255, 255])
        }
        _ => Err(format!("unsupported D3DFORMAT 0x{format:08x} for RGBA8 conversion")),
    }
}

fn build_p8_lut(palette: Option<&[[u8; 4]; 256]>) -> [[u8; 4]; 256] {
    let mut lut = [[0u8; 4]; 256];
    if let Some(pal) = palette {
        let uses_alpha = palette_has_nonzero_alpha(pal);
        for (dst, &entry) in lut.iter_mut().zip(pal.iter()) {
            let a = if uses_alpha { entry[3] } else { 255 };
            *dst = [entry[0], entry[1], entry[2], a];
        }
    } else {
        for (i, dst) in lut.iter_mut().enumerate() {
            let v = i as u8;
            *dst = [v, v, v, 255];
        }
    }
    lut
}

fn check_rect_bounds(
    src: &[u8],
    effective_pitch: usize,
    ry: usize,
    rh: usize,
    row_end_byte: usize,
) -> Result<(), String> {
    if rh == 0 {
        return Ok(());
    }
    let last_row_end = (ry + rh - 1) * effective_pitch + row_end_byte;
    if last_row_end > src.len() {
        return Err(format!(
            "surface buffer too small: need {last_row_end} bytes, got {}",
            src.len()
        ));
    }
    Ok(())
}

#[inline]
fn swizzle_bgra_to_rgba_row(src: &[u8], dst: &mut [u8]) {
    for (px, out) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        let bgra = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
        let rgba = (bgra & 0xFF00_FF00)
            | ((bgra & 0x00FF_0000) >> 16)
            | ((bgra & 0x0000_00FF) << 16);
        out.copy_from_slice(&rgba.to_le_bytes());
    }
}

#[inline]
fn swizzle_bgrx_to_rgba_row(src: &[u8], dst: &mut [u8]) {
    for (px, out) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        let bgrx = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
        let rgba = 0xFF00_0000
            | (bgrx & 0x0000_FF00)
            | ((bgrx & 0x00FF_0000) >> 16)
            | ((bgrx & 0x0000_00FF) << 16);
        out.copy_from_slice(&rgba.to_le_bytes());
    }
}

#[allow(clippy::too_many_arguments)]
pub fn convert_surface_rect_to_rgba8(
    format: u32,
    pitch: u32,
    src: &[u8],
    rx: u32,
    ry: u32,
    rw: u32,
    rh: u32,
    palette: Option<&[[u8; 4]; 256]>,
) -> Result<DecodedRgba8, String> {
    convert_surface_rect_to_rgba8_into(format, pitch, src, rx, ry, rw, rh, palette, Vec::new())
}

#[allow(clippy::too_many_arguments)]
pub fn convert_surface_rect_to_rgba8_into(
    format: u32,
    pitch: u32,
    src: &[u8],
    rx: u32,
    ry: u32,
    rw: u32,
    rh: u32,
    palette: Option<&[[u8; 4]; 256]>,
    mut pixels: Vec<u8>,
) -> Result<DecodedRgba8, String> {
    if rw == 0 || rh == 0 {
        pixels.clear();
        return Ok(DecodedRgba8 {
            width: rw,
            height: rh,
            pixels,
        });
    }

    let bpp = format_bytes_per_pixel(format)
        .ok_or_else(|| format!("unsupported D3DFORMAT 0x{format:08x} for RGBA8 conversion"))?
        as usize;
    let rx_usize = rx as usize;
    let ry_usize = ry as usize;
    let rw_usize = rw as usize;
    let rh_usize = rh as usize;
    let row_end_byte = (rx_usize + rw_usize) * bpp;
    let min_row_bytes = surface_row_pitch(format, rx + rw) as usize;
    let effective_pitch = (pitch as usize).max(min_row_bytes);
    check_rect_bounds(src, effective_pitch, ry_usize, rh_usize, row_end_byte)?;

    let out_stride = rw_usize * RGBA8_BPP;
    let needed = rh_usize * out_stride;
    pixels.resize(needed, 0);
    let src_x_off = rx_usize * bpp;
    let src_span = rw_usize * bpp;
    let flat_contiguous = effective_pitch == src_span && rx_usize == 0;

    match format {
        D3DFMT_A8R8G8B8 => {
            if flat_contiguous {
                let flat_src = &src[ry_usize * effective_pitch..][..rh_usize * src_span];
                swizzle_bgra_to_rgba_row(flat_src, &mut pixels[..needed]);
            } else {
                for dy in 0..rh_usize {
                    let row = &src[(ry_usize + dy) * effective_pitch + src_x_off..][..src_span];
                    let out_row = &mut pixels[dy * out_stride..][..out_stride];
                    swizzle_bgra_to_rgba_row(row, out_row);
                }
            }
        }
        D3DFMT_X8R8G8B8 => {
            if flat_contiguous {
                let flat_src = &src[ry_usize * effective_pitch..][..rh_usize * src_span];
                swizzle_bgrx_to_rgba_row(flat_src, &mut pixels[..needed]);
            } else {
                for dy in 0..rh_usize {
                    let row = &src[(ry_usize + dy) * effective_pitch + src_x_off..][..src_span];
                    let out_row = &mut pixels[dy * out_stride..][..out_stride];
                    swizzle_bgrx_to_rgba_row(row, out_row);
                }
            }
        }
        D3DFMT_A4R4G4B4 => {
            for dy in 0..rh_usize {
                let row = &src[(ry_usize + dy) * effective_pitch + src_x_off..][..src_span];
                let out_row = &mut pixels[dy * out_stride..][..out_stride];
                for (px, dst) in row.chunks_exact(2).zip(out_row.chunks_exact_mut(4)) {
                    let val = u16::from_le_bytes([px[0], px[1]]);
                    dst[0] = (((val >> 8) & 0xF) as u8) * 17;
                    dst[1] = (((val >> 4) & 0xF) as u8) * 17;
                    dst[2] = ((val & 0xF) as u8) * 17;
                    dst[3] = (((val >> 12) & 0xF) as u8) * 17;
                }
            }
        }
        D3DFMT_R5G6B5 => {
            for dy in 0..rh_usize {
                let row = &src[(ry_usize + dy) * effective_pitch + src_x_off..][..src_span];
                let out_row = &mut pixels[dy * out_stride..][..out_stride];
                for (px, dst) in row.chunks_exact(2).zip(out_row.chunks_exact_mut(4)) {
                    let val = u16::from_le_bytes([px[0], px[1]]);
                    let rgb = rgb565_to_rgb8(val);
                    dst[0] = rgb[0];
                    dst[1] = rgb[1];
                    dst[2] = rgb[2];
                    dst[3] = 255;
                }
            }
        }
        D3DFMT_A1R5G5B5 => {
            for dy in 0..rh_usize {
                let row = &src[(ry_usize + dy) * effective_pitch + src_x_off..][..src_span];
                let out_row = &mut pixels[dy * out_stride..][..out_stride];
                for (px, dst) in row.chunks_exact(2).zip(out_row.chunks_exact_mut(4)) {
                    let val = u16::from_le_bytes([px[0], px[1]]);
                    dst[0] = expand_5bit(((val >> 10) & 0x1F) as u8);
                    dst[1] = expand_5bit(((val >> 5) & 0x1F) as u8);
                    dst[2] = expand_5bit((val & 0x1F) as u8);
                    dst[3] = if (val & 0x8000) == 0 { 0 } else { 255 };
                }
            }
        }
        D3DFMT_P8 => {
            let lut = build_p8_lut(palette);
            for dy in 0..rh_usize {
                let row = &src[(ry_usize + dy) * effective_pitch + src_x_off..][..src_span];
                let out_row = &mut pixels[dy * out_stride..][..out_stride];
                for (&idx, dst) in row.iter().zip(out_row.chunks_exact_mut(4)) {
                    dst.copy_from_slice(&lut[idx as usize]);
                }
            }
        }
        D3DFMT_A8 => {
            for dy in 0..rh_usize {
                let row = &src[(ry_usize + dy) * effective_pitch + src_x_off..][..src_span];
                let out_row = &mut pixels[dy * out_stride..][..out_stride];
                for (&alpha, dst) in row.iter().zip(out_row.chunks_exact_mut(4)) {
                    dst[0] = 255;
                    dst[1] = 255;
                    dst[2] = 255;
                    dst[3] = alpha;
                }
            }
        }
        D3DFMT_L8 => {
            for dy in 0..rh_usize {
                let row = &src[(ry_usize + dy) * effective_pitch + src_x_off..][..src_span];
                let out_row = &mut pixels[dy * out_stride..][..out_stride];
                for (&lum, dst) in row.iter().zip(out_row.chunks_exact_mut(4)) {
                    dst[0] = lum;
                    dst[1] = lum;
                    dst[2] = lum;
                    dst[3] = 255;
                }
            }
        }
        _ => {
            let pal_uses_alpha = palette.is_some_and(palette_has_nonzero_alpha);
            for dy in 0..rh_usize {
                let row_start = (ry_usize + dy) * effective_pitch;
                let row = &src[row_start..row_start + row_end_byte];
                let out_row = &mut pixels[dy * out_stride..][..out_stride];
                for (dx, dst) in out_row.chunks_exact_mut(4).enumerate() {
                    let rgba = convert_uncompressed_pixel(
                        format,
                        row,
                        rx_usize + dx,
                        palette,
                        pal_uses_alpha,
                    )?;
                    dst.copy_from_slice(&rgba);
                }
            }
        }
    }

    Ok(DecodedRgba8 {
        width: rw,
        height: rh,
        pixels,
    })
}

pub fn convert_surface_to_rgba8(
    format: u32,
    width: u32,
    height: u32,
    pitch: u32,
    src: &[u8],
    palette: Option<&[[u8; 4]; 256]>,
) -> Result<DecodedRgba8, String> {
    convert_surface_to_rgba8_into(format, width, height, pitch, src, palette, Vec::new())
}

pub fn convert_surface_to_rgba8_into(
    format: u32,
    width: u32,
    height: u32,
    pitch: u32,
    src: &[u8],
    palette: Option<&[[u8; 4]; 256]>,
    pixels: Vec<u8>,
) -> Result<DecodedRgba8, String> {
    match format {
        D3DFMT_DXT1 => return Ok(decode_dxt1_to_rgba8_into(src, width, height, pixels)),
        D3DFMT_DXT2 => return Ok(decode_dxt3_internal(src, width, height, true, pixels)),
        D3DFMT_DXT3 => return Ok(decode_dxt3_internal(src, width, height, false, pixels)),
        D3DFMT_DXT4 => return Ok(decode_dxt5_internal(src, width, height, true, pixels)),
        D3DFMT_DXT5 => return Ok(decode_dxt5_internal(src, width, height, false, pixels)),
        _ => {}
    }

    convert_surface_rect_to_rgba8_into(format, pitch, src, 0, 0, width, height, palette, pixels)
}

fn downsample_rgba8_half(img: &DecodedRgba8) -> DecodedRgba8 {
    let new_w = (img.width / 2).max(1);
    let new_h = (img.height / 2).max(1);
    let mut out = vec![0u8; (new_w as usize) * (new_h as usize) * RGBA8_BPP];

    let src_w = img.width as usize;
    let src_h = img.height as usize;

    for y in 0..new_h as usize {
        let sy0 = (y * 2).min(src_h - 1);
        let sy1 = (y * 2 + 1).min(src_h - 1);
        for x in 0..new_w as usize {
            let sx0 = (x * 2).min(src_w - 1);
            let sx1 = (x * 2 + 1).min(src_w - 1);
            let coords = [(sx0, sy0), (sx1, sy0), (sx0, sy1), (sx1, sy1)];

            let mut sums = [0u32; 4];
            for &(cx, cy) in &coords {
                let idx = (cy * src_w + cx) * RGBA8_BPP;
                for (c, sum) in sums.iter_mut().enumerate() {
                    *sum += u32::from(img.pixels[idx + c]);
                }
            }
            let dst_idx = (y * (new_w as usize) + x) * RGBA8_BPP;
            for (c, &sum) in sums.iter().enumerate() {
                out[dst_idx + c] = (sum / 4) as u8;
            }
        }
    }

    DecodedRgba8 {
        width: new_w,
        height: new_h,
        pixels: out,
    }
}

pub fn clamp_rgba8_to_max_size(mut img: DecodedRgba8, max_size: u32) -> DecodedRgba8 {
    let limit = max_size.max(1);
    while img.width > limit || img.height > limit {
        let prev_w = img.width;
        let prev_h = img.height;
        img = downsample_rgba8_half(&img);
        if img.width == prev_w && img.height == prev_h {
            break;
        }
    }
    img
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_texture_upload(
    format: u32,
    width: u32,
    height: u32,
    pitch: u32,
    src: &[u8],
    palette: Option<&[[u8; 4]; 256]>,
    max_texture_size: u32,
    supports_s3tc: bool,
) -> Result<PreparedTextureUpload, String> {
    prepare_texture_upload_with_buf(
        format,
        width,
        height,
        pitch,
        src,
        palette,
        max_texture_size,
        supports_s3tc,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_texture_upload_with_buf(
    format: u32,
    width: u32,
    height: u32,
    pitch: u32,
    src: &[u8],
    palette: Option<&[[u8; 4]; 256]>,
    max_texture_size: u32,
    supports_s3tc: bool,
    mut scratch_buf: Vec<u8>,
) -> Result<PreparedTextureUpload, String> {
    let limit = max_texture_size.max(1);
    let native_s3tc = supports_s3tc
        && matches!(format, D3DFMT_DXT1 | D3DFMT_DXT3 | D3DFMT_DXT5)
        && width <= limit
        && height <= limit;

    if native_s3tc {
        scratch_buf.resize(src.len(), 0);
        scratch_buf.copy_from_slice(src);
        return Ok(PreparedTextureUpload::CompressedS3tc {
            format,
            width,
            height,
            data: scratch_buf,
        });
    }

    let decoded =
        convert_surface_to_rgba8_into(format, width, height, pitch, src, palette, scratch_buf)?;
    Ok(PreparedTextureUpload::Rgba8(clamp_rgba8_to_max_size(
        decoded, limit,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice_rgba_rect(full: &DecodedRgba8, rx: u32, ry: u32, rw: u32, rh: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((rw * rh * 4) as usize);
        let full_stride = (full.width * 4) as usize;
        let rect_stride = (rw * 4) as usize;
        let x_off = (rx * 4) as usize;
        for dy in 0..rh as usize {
            let start = ((ry as usize) + dy) * full_stride + x_off;
            out.extend_from_slice(&full.pixels[start..start + rect_stride]);
        }
        out
    }

    #[test]
    fn convert_surface_rect_matches_full_surface_slice() {
        let (w, h) = (8u32, 6u32);
        let (rx, ry, rw, rh) = (2u32, 1u32, 4u32, 3u32);

        let argb_pitch = w * 4;
        let argb_src: Vec<u8> = (0..(argb_pitch * h))
            .map(|i| ((i * 37 + 13) & 0xFF) as u8)
            .collect();
        let full_argb =
            convert_surface_to_rgba8(D3DFMT_A8R8G8B8, w, h, argb_pitch, &argb_src, None).unwrap();
        let rect_argb =
            convert_surface_rect_to_rgba8(D3DFMT_A8R8G8B8, argb_pitch, &argb_src, rx, ry, rw, rh, None)
                .unwrap();
        assert_eq!(rect_argb.width, rw);
        assert_eq!(rect_argb.height, rh);
        assert_eq!(rect_argb.pixels, slice_rgba_rect(&full_argb, rx, ry, rw, rh));

        let a4_pitch = w * 2;
        let a4_src: Vec<u8> = (0..(a4_pitch * h))
            .map(|i| ((i * 53 + 7) & 0xFF) as u8)
            .collect();
        let full_a4 =
            convert_surface_to_rgba8(D3DFMT_A4R4G4B4, w, h, a4_pitch, &a4_src, None).unwrap();
        let rect_a4 =
            convert_surface_rect_to_rgba8(D3DFMT_A4R4G4B4, a4_pitch, &a4_src, rx, ry, rw, rh, None)
                .unwrap();
        assert_eq!(rect_a4.pixels, slice_rgba_rect(&full_a4, rx, ry, rw, rh));

        let mut palette = [[0u8; 4]; 256];
        for (i, entry) in palette.iter_mut().enumerate() {
            *entry = [i as u8, 255 - i as u8, (i as u8).wrapping_mul(3), (i as u8) | 1];
        }
        let p8_pitch = w;
        let p8_src: Vec<u8> = (0..(p8_pitch * h))
            .map(|i| ((i * 19 + 5) & 0xFF) as u8)
            .collect();
        let full_p8 =
            convert_surface_to_rgba8(D3DFMT_P8, w, h, p8_pitch, &p8_src, Some(&palette)).unwrap();
        let rect_p8 = convert_surface_rect_to_rgba8(
            D3DFMT_P8,
            p8_pitch,
            &p8_src,
            rx,
            ry,
            rw,
            rh,
            Some(&palette),
        )
        .unwrap();
        assert_eq!(rect_p8.pixels, slice_rgba_rect(&full_p8, rx, ry, rw, rh));
    }

    #[test]
    fn prepare_texture_upload_with_buf_reuses_capacity_and_handles_packed_strided_argb() {
        let scratch = Vec::with_capacity(4096);
        let orig_ptr = scratch.as_ptr();
        let orig_cap = scratch.capacity();

        // Packed A8R8G8B8 (hits flat contiguous fast path)
        let w = 8u32;
        let h = 4u32;
        let packed_src: Vec<u8> = (0..(w * h * 4))
            .map(|i| ((i * 41 + 9) & 0xFF) as u8)
            .collect();
        let prepared_a = prepare_texture_upload_with_buf(
            D3DFMT_A8R8G8B8,
            w,
            h,
            w * 4,
            &packed_src,
            None,
            2048,
            false,
            scratch,
        )
        .unwrap();
        let PreparedTextureUpload::Rgba8(img_a) = prepared_a else {
            panic!("expected Rgba8");
        };
        assert_eq!(img_a.pixels.as_ptr(), orig_ptr);
        assert_eq!(img_a.pixels.capacity(), orig_cap);
        for (src_px, dst_px) in packed_src.chunks_exact(4).zip(img_a.pixels.chunks_exact(4)) {
            assert_eq!(dst_px, &[src_px[2], src_px[1], src_px[0], src_px[3]]);
        }

        // Strided X8R8G8B8 (hits row-by-row swizzle path and forces alpha=255)
        let strided_pitch = w * 4 + 16;
        let mut strided_src = vec![0x77u8; (strided_pitch * h) as usize];
        for row in 0..h as usize {
            let src_row = &packed_src[row * (w as usize * 4)..][..(w as usize * 4)];
            let dst_row = &mut strided_src[row * (strided_pitch as usize)..][..(w as usize * 4)];
            dst_row.copy_from_slice(src_row);
        }
        let prepared_x = prepare_texture_upload_with_buf(
            D3DFMT_X8R8G8B8,
            w,
            h,
            strided_pitch,
            &strided_src,
            None,
            2048,
            false,
            img_a.pixels,
        )
        .unwrap();
        let PreparedTextureUpload::Rgba8(img_x) = prepared_x else {
            panic!("expected Rgba8");
        };
        assert_eq!(img_x.pixels.as_ptr(), orig_ptr);
        assert_eq!(img_x.pixels.capacity(), orig_cap);
        for (src_px, dst_px) in packed_src.chunks_exact(4).zip(img_x.pixels.chunks_exact(4)) {
            assert_eq!(dst_px, &[src_px[2], src_px[1], src_px[0], 255]);
        }

        // Packed X8R8G8B8 + sub-rect A8R8G8B8 into same buffer
        let expected_x = img_x.pixels.clone();
        let packed_x = convert_surface_rect_to_rgba8_into(
            D3DFMT_X8R8G8B8,
            w * 4,
            &packed_src,
            0,
            0,
            w,
            h,
            None,
            img_x.pixels,
        )
        .unwrap();
        assert_eq!(packed_x.pixels.as_ptr(), orig_ptr);
        assert_eq!(packed_x.pixels, expected_x);
    }
}
