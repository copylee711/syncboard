//! Renders the app icon (two overlapping boards on a gradient tile).
//!   cargo run --example gen_icon -- <out_dir>

use image::{ImageBuffer, Rgba, RgbaImage};

/// Signed distance to a rounded rectangle centered at (cx, cy).
fn rrect(x: f32, y: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let dx = (x - cx).abs() - (hw - r);
    let dy = (y - cy).abs() - (hh - r);
    let ox = dx.max(0.0);
    let oy = dy.max(0.0);
    (ox * ox + oy * oy).sqrt() + dx.max(dy).min(0.0) - r
}

fn coverage(d: f32, px: f32) -> f32 {
    (0.5 - d / px).clamp(0.0, 1.0)
}

fn blend(dst: [f32; 4], src: [f32; 3], a: f32) -> [f32; 4] {
    let out_a = a + dst[3] * (1.0 - a);
    if out_a <= 0.0 {
        return [0.0; 4];
    }
    let mut o = [0.0; 4];
    for i in 0..3 {
        o[i] = (src[i] * a + dst[i] * dst[3] * (1.0 - a)) / out_a;
    }
    o[3] = out_a;
    o
}

fn render(size: u32) -> RgbaImage {
    let s = size as f32;
    let px = 1.0 / s; // one pixel in unit space
    ImageBuffer::from_fn(size, size, |ix, iy| {
        let x = (ix as f32 + 0.5) / s;
        let y = (iy as f32 + 0.5) / s;
        let mut c = [0.0f32; 4];
        // Ink tile, a green back board and a paper front board.
        let ink = [0.11, 0.11, 0.10];
        let green = [0.27, 0.72, 0.52];
        let paper = [0.96, 0.95, 0.93];
        c = blend(c, ink, coverage(rrect(x, y, 0.5, 0.5, 0.46, 0.46, 0.22), px));
        c = blend(c, green, coverage(rrect(x, y, 0.59, 0.41, 0.19, 0.23, 0.05), px));
        c = blend(c, paper, coverage(rrect(x, y, 0.41, 0.59, 0.19, 0.23, 0.05), px));
        for (ly, w) in [(0.53, 0.10), (0.63, 0.06)] {
            let d = rrect(x, y, 0.41 - (0.10 - w), ly, w, 0.02, 0.02);
            c = blend(c, ink, coverage(d, px));
        }
        Rgba([(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (c[3] * 255.0) as u8])
    })
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("out dir"));
    std::fs::create_dir_all(&out).unwrap();
    for size in [32u32, 128, 256] {
        render(size).save(out.join(format!("{size}x{size}.png"))).unwrap();
    }
    render(512).save(out.join("icon.png")).unwrap();
    // Multi-size ICO for the exe and tray.
    let mut ico = Vec::new();
    {
        let enc = image::codecs::ico::IcoEncoder::new(&mut ico);
        let frames: Vec<_> = [16u32, 24, 32, 48, 64, 256]
            .iter()
            .map(|&s| {
                let img = render(s);
                image::codecs::ico::IcoFrame::as_png(img.as_raw(), s, s, image::ExtendedColorType::Rgba8).unwrap()
            })
            .collect();
        enc.encode_images(&frames).unwrap();
    }
    std::fs::write(out.join("icon.ico"), ico).unwrap();
    println!("icons written to {}", out.display());
}
