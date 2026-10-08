//! Turns whatever the AI (or the user) produced into a clean, print-ready colouring page:
//! pure black lines on pure white, no specks, no grey shading, no solid black blobs,
//! gaps closed so regions can be coloured/filled, and line weight suited to the age band.

use anyhow::Result;
use image::{imageops::FilterType, DynamicImage, GrayImage, Luma};
use imageproc::contrast::otsu_level;
use imageproc::distance_transform::Norm;
use imageproc::morphology::{dilate, erode};
use imageproc::region_labelling::{connected_components, Connectivity};

use crate::model::AgeBand;

/// Long side of the processed page in pixels: at least 300 dpi for a square picture on
/// US Letter / A4 (KDP's print requirement) and ≈ 300 dpi for a full-page portrait picture.
pub const WORK_LONG_SIDE: u32 = 3072;

#[derive(Debug, Clone, Copy)]
pub struct LineParams {
    /// Morphological closing radius: bridges small gaps in outlines.
    pub close: u8,
    /// Extra line thickness added at the end.
    pub thicken: u8,
    /// Enclosed white areas smaller than this fraction of the page are filled in
    /// (too small to colour; also removes texture noise).
    pub min_white_frac: f64,
    /// Black specks smaller than this fraction of the page are removed.
    pub min_black_frac: f64,
}

/// Parameters tuned for a 2048 px long side; scaled for other sizes.
pub fn params_for(age: AgeBand) -> LineParams {
    match age {
        AgeBand::Early => LineParams { close: 3, thicken: 3, min_white_frac: 0.00008, min_black_frac: 0.00003 },
        AgeBand::Middle => LineParams { close: 2, thicken: 2, min_white_frac: 0.00005, min_black_frac: 0.00002 },
        AgeBand::Older => LineParams { close: 1, thicken: 1, min_white_frac: 0.00003, min_black_frac: 0.00001 },
    }
}

pub fn to_coloring_png(bytes: &[u8], age: AgeBand) -> Result<Vec<u8>> {
    let img = image::load_from_memory(bytes)?;
    let page = process(&img, &params_for(age), WORK_LONG_SIDE);
    let mut out = Vec::new();
    page.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

/// Returns a page where every pixel is 0 (ink) or 255 (paper).
pub fn process(img: &DynamicImage, p: &LineParams, long_side: u32) -> GrayImage {
    // Flatten any transparency onto white, then luminance.
    let rgba = img.to_rgba8();
    let mut gray = GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let l = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        let a = a as f32 / 255.0;
        Luma([(l * a + 255.0 * (1.0 - a)).round() as u8])
    });

    let (w, h) = gray.dimensions();
    let scale = long_side as f32 / w.max(h) as f32;
    if (scale - 1.0).abs() > 0.01 {
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        gray = image::imageops::resize(&gray, nw, nh, FilterType::CatmullRom);
    }
    let (w, h) = gray.dimensions();
    let area = (w as f64) * (h as f64);
    // Radii were tuned at 2048 px.
    let k = |r: u8| -> u8 { ((r as f32 * long_side as f32 / 2048.0).round() as u8).max(if r > 0 { 1 } else { 0 }) };

    // Light grey shading becomes paper; dark grey becomes ink.
    let t = otsu_level(&gray).clamp(90, 170);
    let mut ink = GrayImage::from_fn(w, h, |x, y| Luma([if gray.get_pixel(x, y)[0] < t { 255 } else { 0 }]));

    remove_small(&mut ink, (p.min_black_frac * area) as u32, Connectivity::Eight);

    if p.close > 0 {
        ink = erode(&dilate(&ink, Norm::LInf, k(p.close)), Norm::LInf, k(p.close));
    }

    // Hollow out solid black areas (hair, shadows): keep only an outline of width r.
    let r = ((long_side / 170) as u8).max(2);
    let core = erode(&ink, Norm::LInf, r);
    for (px, c) in ink.pixels_mut().zip(core.pixels()) {
        if c[0] == 255 {
            px[0] = 0;
        }
    }

    if p.thicken > 0 {
        ink = dilate(&ink, Norm::LInf, k(p.thicken));
    }

    // Paper view; tiny enclosed white pockets become ink.
    let mut paper = GrayImage::from_fn(w, h, |x, y| Luma([255 - ink.get_pixel(x, y)[0]]));
    remove_small(&mut paper, (p.min_white_frac * area) as u32, Connectivity::Four);
    paper
}

/// Places character reference pictures side by side on one white sheet (PNG), so a
/// single reference image can carry several characters.
pub fn reference_sheet(images: &[GrayImage]) -> Result<Vec<u8>> {
    const H: u32 = 768;
    const GAP: u32 = 48;
    let scaled: Vec<GrayImage> = images
        .iter()
        .map(|img| {
            let w = ((img.width() as f32 * H as f32 / img.height().max(1) as f32).round() as u32).max(1);
            image::imageops::resize(img, w, H, FilterType::Triangle)
        })
        .collect();
    let width = scaled.iter().map(|i| i.width()).sum::<u32>() + GAP * (scaled.len() as u32 + 1);
    let mut sheet = GrayImage::from_pixel(width.max(1), H + 2 * GAP, Luma([255]));
    let mut x = GAP;
    for img in &scaled {
        image::imageops::replace(&mut sheet, img, x as i64, GAP as i64);
        x += img.width() + GAP;
    }
    let mut out = Vec::new();
    sheet.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

/// Clears (sets to 0) connected foreground regions smaller than `min_area` pixels.
fn remove_small(mask: &mut GrayImage, min_area: u32, conn: Connectivity) {
    if min_area == 0 {
        return;
    }
    let labels = connected_components(&*mask, conn, Luma([0u8]));
    let mut sizes: Vec<u32> = Vec::new();
    for l in labels.pixels() {
        let l = l[0] as usize;
        if l >= sizes.len() {
            sizes.resize(l + 1, 0);
        }
        sizes[l] += 1;
    }
    for (px, l) in mask.pixels_mut().zip(labels.pixels()) {
        let l = l[0] as usize;
        if l != 0 && sizes[l] < min_area {
            px[0] = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imageproc::drawing::{draw_filled_rect_mut, draw_hollow_circle_mut};
    use imageproc::rect::Rect;

    fn run(img: GrayImage) -> GrayImage {
        process(&DynamicImage::ImageLuma8(img), &params_for(AgeBand::Middle), 400)
    }

    fn blank() -> GrayImage {
        GrayImage::from_pixel(400, 400, Luma([255]))
    }

    #[test]
    fn output_is_pure_black_and_white() {
        let mut img = blank();
        for (i, p) in img.pixels_mut().enumerate() {
            p[0] = (i % 251) as u8;
        }
        let out = run(img);
        assert!(out.pixels().all(|p| p[0] == 0 || p[0] == 255));
    }

    #[test]
    fn specks_removed_outlines_kept() {
        let mut img = blank();
        img.put_pixel(50, 50, Luma([0]));
        for d in 0..3 {
            draw_hollow_circle_mut(&mut img, (200, 200), 120 - d, Luma([0]));
        }
        let out = run(img);
        assert_eq!(out.get_pixel(50, 50)[0], 255, "speck should be gone");
        assert_eq!(out.get_pixel(200, 81)[0], 0, "outline should remain");
        assert_eq!(out.get_pixel(200, 200)[0], 255, "inside stays colourable");
    }

    #[test]
    fn light_grey_shading_becomes_white() {
        let mut img = blank();
        draw_filled_rect_mut(&mut img, Rect::at(100, 100).of_size(200, 200), Luma([215]));
        for d in 0..3 {
            draw_hollow_circle_mut(&mut img, (200, 200), 150 - d, Luma([0]));
        }
        let out = run(img);
        assert_eq!(out.get_pixel(200, 200)[0], 255);
    }

    #[test]
    fn solid_black_area_is_hollowed() {
        let mut img = blank();
        draw_filled_rect_mut(&mut img, Rect::at(100, 100).of_size(200, 200), Luma([0]));
        let out = run(img);
        assert_eq!(out.get_pixel(200, 200)[0], 255, "centre of a fill becomes colourable");
        assert_eq!(out.get_pixel(101, 200)[0], 0, "edge of the fill remains as outline");
    }

    #[test]
    fn tiny_white_pockets_filled() {
        let mut img = blank();
        // 3×3 white hole inside a 9×9 black square: noise, too small to colour.
        draw_filled_rect_mut(&mut img, Rect::at(50, 50).of_size(9, 9), Luma([0]));
        draw_filled_rect_mut(&mut img, Rect::at(53, 53).of_size(3, 3), Luma([255]));
        let out = run(img);
        assert_eq!(out.get_pixel(54, 54)[0], 0);
    }

    #[test]
    fn reference_sheet_tiles_side_by_side() {
        let a = GrayImage::from_pixel(300, 400, Luma([0]));
        let b = GrayImage::from_pixel(600, 400, Luma([0]));
        let sheet = image::load_from_memory(&reference_sheet(&[a, b]).unwrap()).unwrap().to_luma8();
        assert_eq!(sheet.height(), 768 + 96);
        assert_eq!(sheet.width(), 576 + 1152 + 48 * 3);
        assert_eq!(sheet.get_pixel(10, 10)[0], 255);
        assert_eq!(sheet.get_pixel(48 + 10, 100)[0], 0);
    }

    #[test]
    fn small_gaps_are_closed() {
        // Circle outline with a 2 px gap: the inside must not leak into the background.
        let mut img = blank();
        for d in 0..3 {
            draw_hollow_circle_mut(&mut img, (200, 200), 100 - d, Luma([0]));
        }
        draw_filled_rect_mut(&mut img, Rect::at(296, 199).of_size(8, 2), Luma([255]));
        let out = run(img);
        let labels = connected_components(&out, Connectivity::Four, Luma([0u8]));
        assert_ne!(labels.get_pixel(200, 200)[0], labels.get_pixel(5, 5)[0]);
    }
}
