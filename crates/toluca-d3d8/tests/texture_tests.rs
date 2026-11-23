use toluca_d3d8::texture::{
    DecodedRgba8, PreparedTextureUpload, clamp_rgba8_to_max_size, convert_surface_to_rgba8,
    decode_dxt1_to_rgba8, decode_dxt3_to_rgba8, decode_dxt5_to_rgba8, prepare_texture_upload,
};
use toluca_d3d8::types::{
    D3DFMT_A1R5G5B5, D3DFMT_A4R4G4B4, D3DFMT_A8, D3DFMT_A8L8, D3DFMT_A8R8G8B8, D3DFMT_DXT1,
    D3DFMT_DXT3, D3DFMT_DXT5, D3DFMT_L8, D3DFMT_P8, D3DFMT_R5G6B5, D3DFMT_R8G8B8, D3DFMT_V8U8,
    D3DFMT_X1R5G5B5, D3DFMT_X8R8G8B8,
};

fn pack_rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((u16::from(r) & 0x1F) << 11) | ((u16::from(g) & 0x3F) << 5) | (u16::from(b) & 0x1F)
}

#[test]
fn dxt1_opaque_4_color_interpolation() {
    let c0 = pack_rgb565(31, 0, 0);
    let c1 = pack_rgb565(0, 0, 31);
    assert!(c0 > c1);

    let mut block = [0u8; 8];
    block[0..2].copy_from_slice(&c0.to_le_bytes());
    block[2..4].copy_from_slice(&c1.to_le_bytes());
    let indices: u32 = (1 << 2) | (2 << 4) | (3 << 6);
    block[4..8].copy_from_slice(&indices.to_le_bytes());

    let img = decode_dxt1_to_rgba8(&block, 4, 4);
    assert_eq!(img.width, 4);
    assert_eq!(img.height, 4);
    assert_eq!(img.pixels.len(), 64);

    assert_eq!(&img.pixels[0..4], &[255, 0, 0, 255]);
    assert_eq!(&img.pixels[4..8], &[0, 0, 255, 255]);
    assert_eq!(&img.pixels[8..12], &[170, 0, 85, 255]);
    assert_eq!(&img.pixels[12..16], &[85, 0, 170, 255]);
}

#[test]
fn dxt1_transparent_and_non_multiple_of_4_mips() {
    let c0 = pack_rgb565(0, 63, 0);
    let c1 = pack_rgb565(31, 63, 31);
    assert!(c0 <= c1);

    let mut block = [0u8; 8];
    block[0..2].copy_from_slice(&c0.to_le_bytes());
    block[2..4].copy_from_slice(&c1.to_le_bytes());
    let indices: u32 = (2 << 2) | (3 << 8) | (1 << 10);
    block[4..8].copy_from_slice(&indices.to_le_bytes());

    let img_2x2 = decode_dxt1_to_rgba8(&block, 2, 2);
    assert_eq!(img_2x2.width, 2);
    assert_eq!(img_2x2.height, 2);
    assert_eq!(img_2x2.pixels.len(), 16);
    assert_eq!(&img_2x2.pixels[0..4], &[0, 255, 0, 255]);
    assert_eq!(&img_2x2.pixels[4..8], &[127, 255, 127, 255]);
    assert_eq!(&img_2x2.pixels[8..12], &[0, 0, 0, 0]);
    assert_eq!(&img_2x2.pixels[12..16], &[255, 255, 255, 255]);

    let img_1x1 = decode_dxt1_to_rgba8(&block, 1, 1);
    assert_eq!(img_1x1.pixels, vec![0, 255, 0, 255]);
}

#[test]
fn dxt3_explicit_4bit_alpha_and_color() {
    let mut block = [0u8; 16];
    let alpha_bits: u64 = (0x8u64 << 4) | (0xFu64 << 8);
    block[0..8].copy_from_slice(&alpha_bits.to_le_bytes());

    let c0 = pack_rgb565(0, 0, 31);
    let c1 = pack_rgb565(31, 0, 0);
    block[8..10].copy_from_slice(&c0.to_le_bytes());
    block[10..12].copy_from_slice(&c1.to_le_bytes());
    let indices: u32 = (1 << 2) | (2 << 4) | (3 << 6);
    block[12..16].copy_from_slice(&indices.to_le_bytes());

    let img = decode_dxt3_to_rgba8(&block, 4, 4);
    assert_eq!(&img.pixels[0..4], &[0, 0, 255, 0]);
    assert_eq!(&img.pixels[4..8], &[255, 0, 0, 136]);
    assert_eq!(&img.pixels[8..12], &[85, 0, 170, 255]);
    assert_eq!(&img.pixels[12..16], &[170, 0, 85, 0]);
}

#[test]
fn dxt5_eight_value_and_six_value_alpha_modes() {
    let mut block_8 = [0u8; 16];
    block_8[0] = 210;
    block_8[1] = 0;
    let alpha_idx_8: u64 = (1 << 3) | (2 << 6) | (7 << 9);
    block_8[2..8].copy_from_slice(&alpha_idx_8.to_le_bytes()[0..6]);
    let white = pack_rgb565(31, 63, 31);
    block_8[8..10].copy_from_slice(&white.to_le_bytes());
    block_8[10..12].copy_from_slice(&white.to_le_bytes());

    let img_8 = decode_dxt5_to_rgba8(&block_8, 4, 4);
    assert_eq!(img_8.pixels[3], 210);
    assert_eq!(img_8.pixels[7], 0);
    assert_eq!(img_8.pixels[11], 180);
    assert_eq!(img_8.pixels[15], 30);

    let mut block_6 = [0u8; 16];
    block_6[0] = 50;
    block_6[1] = 200;
    let alpha_idx_6: u64 = (1 << 3) | (2 << 6) | (6 << 9) | (7 << 12);
    block_6[2..8].copy_from_slice(&alpha_idx_6.to_le_bytes()[0..6]);
    block_6[8..10].copy_from_slice(&white.to_le_bytes());
    block_6[10..12].copy_from_slice(&white.to_le_bytes());

    let img_6 = decode_dxt5_to_rgba8(&block_6, 4, 4);
    assert_eq!(img_6.pixels[3], 50);
    assert_eq!(img_6.pixels[7], 200);
    assert_eq!(img_6.pixels[11], 80);
    assert_eq!(img_6.pixels[15], 0);
    assert_eq!(img_6.pixels[19], 255);
}

#[test]
fn uncompressed_d3dformats_conversion() {
    let a8r8g8b8 = [0x11, 0x22, 0x33, 0x44];
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_A8R8G8B8, 1, 1, 4, &a8r8g8b8, None)
            .unwrap()
            .pixels,
        vec![0x33, 0x22, 0x11, 0x44]
    );

    let x8r8g8b8 = [0x11, 0x22, 0x33, 0x00];
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_X8R8G8B8, 1, 1, 4, &x8r8g8b8, None)
            .unwrap()
            .pixels,
        vec![0x33, 0x22, 0x11, 255]
    );

    let r8g8b8 = [0x10, 0x20, 0x30];
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_R8G8B8, 1, 1, 3, &r8g8b8, None)
            .unwrap()
            .pixels,
        vec![0x30, 0x20, 0x10, 255]
    );

    let r5g6b5 = pack_rgb565(31, 0, 31).to_le_bytes();
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_R5G6B5, 1, 1, 2, &r5g6b5, None)
            .unwrap()
            .pixels,
        vec![255, 0, 255, 255]
    );

    let a1r5g5b5 = (0x8000u16 | (31 << 10)).to_le_bytes();
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_A1R5G5B5, 1, 1, 2, &a1r5g5b5, None)
            .unwrap()
            .pixels,
        vec![255, 0, 0, 255]
    );
    let a1_zero = (31u16).to_le_bytes();
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_A1R5G5B5, 1, 1, 2, &a1_zero, None)
            .unwrap()
            .pixels,
        vec![0, 0, 255, 0]
    );
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_X1R5G5B5, 1, 1, 2, &a1_zero, None)
            .unwrap()
            .pixels,
        vec![0, 0, 255, 255]
    );

    let a4r4g4b4 = 0x8F41u16.to_le_bytes();
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_A4R4G4B4, 1, 1, 2, &a4r4g4b4, None)
            .unwrap()
            .pixels,
        vec![255, 68, 17, 136]
    );

    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_L8, 1, 1, 1, &[128], None)
            .unwrap()
            .pixels,
        vec![128, 128, 128, 255]
    );
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_A8L8, 1, 1, 2, &[90, 200], None)
            .unwrap()
            .pixels,
        vec![90, 90, 90, 200]
    );
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_A8, 1, 1, 1, &[77], None)
            .unwrap()
            .pixels,
        vec![255, 255, 255, 77]
    );

    let mut pal = [[0u8; 4]; 256];
    pal[5] = [10, 20, 30, 0];
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_P8, 1, 1, 1, &[5], Some(&pal))
            .unwrap()
            .pixels,
        vec![10, 20, 30, 255]
    );
    pal[6] = [40, 50, 60, 180];
    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_P8, 1, 1, 1, &[6], Some(&pal))
            .unwrap()
            .pixels,
        vec![40, 50, 60, 180]
    );

    assert_eq!(
        convert_surface_to_rgba8(D3DFMT_V8U8, 1, 1, 2, &[0x80, 0x7F], None)
            .unwrap()
            .pixels,
        vec![0, 255, 255, 255]
    );
}

#[test]
fn clamp_max_size_and_prepare_upload_modes() {
    let img = DecodedRgba8 {
        width: 512,
        height: 256,
        pixels: vec![100u8; 512 * 256 * 4],
    };
    let clamped = clamp_rgba8_to_max_size(img, 128);
    assert_eq!(clamped.width, 128);
    assert_eq!(clamped.height, 64);
    assert_eq!(clamped.pixels.len(), 128 * 64 * 4);
    assert_eq!(&clamped.pixels[0..4], &[100, 100, 100, 100]);

    let dxt1_bytes = vec![0u8; (256 / 4) * (256 / 4) * 8];
    let native =
        prepare_texture_upload(D3DFMT_DXT1, 256, 256, 512, &dxt1_bytes, None, 1024, true).unwrap();
    assert!(matches!(
        native,
        PreparedTextureUpload::CompressedS3tc {
            format: D3DFMT_DXT1,
            width: 256,
            height: 256,
            ..
        }
    ));

    let sw_fallback =
        prepare_texture_upload(D3DFMT_DXT1, 256, 256, 512, &dxt1_bytes, None, 1024, false)
            .unwrap();
    match sw_fallback {
        PreparedTextureUpload::Rgba8(decoded) => {
            assert_eq!(decoded.width, 256);
            assert_eq!(decoded.height, 256);
        }
        PreparedTextureUpload::CompressedS3tc { .. } => {
            panic!("expected software RGBA8 fallback when S3TC is unsupported")
        }
    }

    let dxt3_bytes = vec![0u8; (64 / 4) * (64 / 4) * 16];
    let native_dxt3 =
        prepare_texture_upload(D3DFMT_DXT3, 64, 64, 256, &dxt3_bytes, None, 1024, true).unwrap();
    assert!(matches!(
        native_dxt3,
        PreparedTextureUpload::CompressedS3tc {
            format: D3DFMT_DXT3,
            width: 64,
            height: 64,
            ..
        }
    ));

    let oversized_s3tc =
        prepare_texture_upload(D3DFMT_DXT5, 256, 256, 1024, &vec![0u8; 64 * 64 * 16], None, 128, true)
            .unwrap();
    match oversized_s3tc {
        PreparedTextureUpload::Rgba8(decoded) => {
            assert_eq!(decoded.width, 128);
            assert_eq!(decoded.height, 128);
        }
        PreparedTextureUpload::CompressedS3tc { .. } => {
            panic!("expected software RGBA8 downsample when texture exceeds max_texture_size")
        }
    }
}
