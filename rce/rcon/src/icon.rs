use anyhow::Result;
use std::path::Path;

const W: u32 = 64;
const H: u32 = 64;

#[rustfmt::skip]
const D1: [u8; 35] = [
    0,0,1,0,0,
    0,1,1,0,0,
    0,0,1,0,0,
    0,0,1,0,0,
    0,0,1,0,0,
    0,0,1,0,0,
    0,1,1,1,0,
];

#[rustfmt::skip]
const D3: [u8; 35] = [
    1,1,1,1,0,
    0,0,0,0,1,
    0,0,0,0,1,
    0,1,1,1,0,
    0,0,0,0,1,
    0,0,0,0,1,
    1,1,1,1,0,
];

#[rustfmt::skip]
const D7: [u8; 35] = [
    1,1,1,1,1,
    0,0,0,0,1,
    0,0,0,1,0,
    0,0,1,0,0,
    0,0,1,0,0,
    0,0,1,0,0,
    0,0,1,0,0,
];

fn blit(buf: &mut [u8], bitmap: &[u8], ox: usize, oy: usize, scale: usize) {
    for row in 0..7usize {
        for col in 0..5usize {
            if bitmap[row * 5 + col] == 1 {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let x = ox + col * scale + dx;
                        let y = oy + row * scale + dy;
                        if x < W as usize && y < H as usize {
                            let i = (y * W as usize + x) * 4;
                            buf[i]     = 0x00;
                            buf[i + 1] = 0xff;
                            buf[i + 2] = 0x41;
                            buf[i + 3] = 0xff;
                        }
                    }
                }
            }
        }
    }
}

pub fn write_server_icon(path: &Path) -> Result<()> {
    let mut px = vec![0x11u8; (W * H * 4) as usize];
    for i in 0..(W * H) as usize {
        px[i * 4 + 3] = 0xff;
    }

    let scale = 2usize;
    let dw    = 5 * scale;
    let gap   = 2usize;
    let total_w = 4 * dw + 3 * gap;
    let total_h = 7 * scale;
    let ox = (W as usize - total_w) / 2;
    let oy = (H as usize - total_h) / 2;

    let digits: [&[u8]; 4] = [&D1, &D3, &D3, &D7];
    for (i, bm) in digits.iter().enumerate() {
        blit(&mut px, bm, ox + i * (dw + gap), oy, scale);
    }

    let f = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(f, W, H);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    writer.write_image_data(&px)?;
    Ok(())
}
