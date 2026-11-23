use crate::types::{
    D3DFMT_A1R5G5B5, D3DFMT_A4R4G4B4, D3DFMT_A8, D3DFMT_A8L8, D3DFMT_A8R8G8B8, D3DFMT_DXT1,
    D3DFMT_DXT2, D3DFMT_DXT3, D3DFMT_DXT4, D3DFMT_DXT5, D3DFMT_L8, D3DFMT_P8, D3DFMT_R5G6B5,
    D3DFMT_R8G8B8, D3DFMT_V8U8, D3DFMT_X1R5G5B5, D3DFMT_X4R4G4B4, D3DFMT_X8R8G8B8,
    surface_row_pitch,
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

pub fn decode_dxt1_to_rgba8(src: &[u8], width: u32, height: u32) -> DecodedRgba8 {
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * RGBA8_BPP];
    if width == 0 || height == 0 {
        return DecodedRgba8 {
            width,
            height,
            pixels,
        };
    }

    let blocks_w = width.div_ceil(DXT_BLOCK_DIM);
    let blocks_h = height.div_ceil(DXT_BLOCK_DIM);

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

fn decode_dxt3_internal(
    src: &[u8],
    width: u32,
    height: u32,
    premultiplied: bool,
) -> DecodedRgba8 {
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * RGBA8_BPP];
    if width == 0 || height == 0 {
        return DecodedRgba8 {
            width,
            height,
            pixels,
        };
    }

    let blocks_w = width.div_ceil(DXT_BLOCK_DIM);
    let blocks_h = height.div_ceil(DXT_BLOCK_DIM);

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
    decode_dxt3_internal(src, width, height, false)
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
) -> DecodedRgba8 {
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * RGBA8_BPP];
    if width == 0 || height == 0 {
        return DecodedRgba8 {
            width,
            height,
            pixels,
        };
    }

    let blocks_w = width.div_ceil(DXT_BLOCK_DIM);
    let blocks_h = height.div_ceil(DXT_BLOCK_DIM);

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
    decode_dxt5_internal(src, width, height, false)
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

pub fn convert_surface_to_rgba8(
    format: u32,
    width: u32,
    height: u32,
    pitch: u32,
    src: &[u8],
    palette: Option<&[[u8; 4]; 256]>,
) -> Result<DecodedRgba8, String> {
    match format {
        D3DFMT_DXT1 => return Ok(decode_dxt1_to_rgba8(src, width, height)),
        D3DFMT_DXT2 => return Ok(decode_dxt3_internal(src, width, height, true)),
        D3DFMT_DXT3 => return Ok(decode_dxt3_to_rgba8(src, width, height)),
        D3DFMT_DXT4 => return Ok(decode_dxt5_internal(src, width, height, true)),
        D3DFMT_DXT5 => return Ok(decode_dxt5_to_rgba8(src, width, height)),
        _ => {}
    }

    let mut pixels = vec![0u8; (width as usize) * (height as usize) * RGBA8_BPP];
    if width == 0 || height == 0 {
        return Ok(DecodedRgba8 {
            width,
            height,
            pixels,
        });
    }

    let min_row_bytes = surface_row_pitch(format, width) as usize;
    let effective_pitch = (pitch as usize).max(min_row_bytes);
    let pal_uses_alpha = palette.is_some_and(palette_has_nonzero_alpha);

    for y in 0..height as usize {
        let row_start = y * effective_pitch;
        let row_end = row_start + min_row_bytes;
        if row_end > src.len() {
            return Err(format!(
                "surface buffer too small: need {row_end} bytes, got {}",
                src.len()
            ));
        }
        let row = &src[row_start..row_end];
        for x in 0..width as usize {
            let rgba = convert_uncompressed_pixel(format, row, x, palette, pal_uses_alpha)?;
            let out_idx = (y * (width as usize) + x) * RGBA8_BPP;
            pixels[out_idx..out_idx + RGBA8_BPP].copy_from_slice(&rgba);
        }
    }

    Ok(DecodedRgba8 {
        width,
        height,
        pixels,
    })
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
    let limit = max_texture_size.max(1);
    let native_s3tc = supports_s3tc
        && matches!(format, D3DFMT_DXT1 | D3DFMT_DXT3 | D3DFMT_DXT5)
        && width <= limit
        && height <= limit;

    if native_s3tc {
        return Ok(PreparedTextureUpload::CompressedS3tc {
            format,
            width,
            height,
            data: src.to_vec(),
        });
    }

    let decoded = convert_surface_to_rgba8(format, width, height, pitch, src, palette)?;
    Ok(PreparedTextureUpload::Rgba8(clamp_rgba8_to_max_size(
        decoded, limit,
    )))
}
