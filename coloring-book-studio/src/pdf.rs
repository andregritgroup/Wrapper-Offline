//! Minimal, dependency-light PDF writer for the finished book.
//! Uses the built-in Helvetica fonts (WinAnsi encoding) and 1-bit Flate-compressed images,
//! so a 30-page book stays small and prints razor-sharp.

use std::collections::HashMap;
use std::io::Write;

use flate2::{write::ZlibEncoder, Compression};
use image::GrayImage;

use crate::model::{AgeBand, Layout, Project};

const MARGIN: f32 = 42.0;

struct Pdf {
    objects: Vec<Option<Vec<u8>>>,
}

impl Pdf {
    fn new() -> Self {
        Pdf { objects: Vec::new() }
    }
    fn reserve(&mut self) -> usize {
        self.objects.push(None);
        self.objects.len()
    }
    fn set(&mut self, id: usize, body: Vec<u8>) {
        self.objects[id - 1] = Some(body);
    }
    fn add(&mut self, body: Vec<u8>) -> usize {
        let id = self.reserve();
        self.set(id, body);
        id
    }
    fn add_stream(&mut self, dict: &str, data: &[u8]) -> usize {
        let mut body = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.add(body)
    }
    fn finish(self, root: usize) -> Vec<u8> {
        let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::with_capacity(self.objects.len());
        for (i, obj) in self.objects.into_iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(&obj.expect("unset PDF object"));
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
        for o in &offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1)
                .as_bytes(),
        );
        out
    }
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::best());
    e.write_all(data).expect("in-memory write");
    e.finish().expect("in-memory write")
}

/// Map a char to a WinAnsiEncoding byte (covers Latin-1, so Afrikaans ê/ë/ô etc. work).
fn win_ansi(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '\u{A0}'..='\u{FF}' => c as u32 as u8,
        '\u{2018}' => 0x91,
        '\u{2019}' | '\u{02BC}' => 0x92,
        '\u{201C}' => 0x93,
        '\u{201D}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{2026}' => 0x85,
        '\u{20AC}' => 0x80,
        '\t' => b' ',
        _ => b'?',
    }
}

/// Helvetica advance widths (1/1000 em) for ASCII 32..=126, from the standard AFM.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // space .. /
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0-9
    278, 278, 584, 584, 584, 556, 1015, // : ; < = > ? @
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667,
    611, 722, 667, 944, 667, 667, 611, // A-Z
    278, 278, 278, 469, 556, 333, // [ \ ] ^ _ `
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500,
    278, 556, 500, 722, 500, 500, 500, // a-z
    334, 260, 334, 584, // { | } ~
];

fn char_width(b: u8, bold: bool) -> f32 {
    let w = match b {
        32..=126 => HELVETICA[(b - 32) as usize] as f32,
        0x91 | 0x92 => 222.0,
        0x93 | 0x94 => 333.0,
        0x85 | 0x97 => 1000.0,
        _ => 556.0,
    };
    // Helvetica-Bold is slightly wider; this approximation is only used for centring titles.
    if bold {
        w * 1.07
    } else {
        w
    }
}

fn text_width(s: &str, size: f32, bold: bool) -> f32 {
    s.chars().map(|c| char_width(win_ansi(c), bold)).sum::<f32>() * size / 1000.0
}

/// Word-wrap to `max_w`; paragraph breaks become empty lines.
fn wrap(text: &str, size: f32, max_w: f32, bold: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let normalized = text.replace("\r\n", "\n");
    let paragraphs: Vec<&str> = normalized.split("\n\n").collect();
    for (pi, para) in paragraphs.iter().enumerate() {
        if pi > 0 {
            lines.push(String::new());
        }
        for hard_line in para.split('\n') {
            let mut line = String::new();
            for word in hard_line.split_whitespace() {
                let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                if line.is_empty() || text_width(&candidate, size, bold) <= max_w {
                    line = candidate;
                } else {
                    lines.push(std::mem::take(&mut line));
                    line = word.to_string();
                }
            }
            lines.push(line);
        }
    }
    lines
}

struct Canvas {
    ops: Vec<u8>,
    images: Vec<usize>,
}

impl Canvas {
    fn new() -> Self {
        Canvas { ops: Vec::new(), images: Vec::new() }
    }
    fn text(&mut self, s: &str, size: f32, x: f32, y: f32, bold: bool) {
        let font = if bold { "F2" } else { "F1" };
        self.ops.extend_from_slice(format!("BT /{font} {size:.2} Tf {x:.2} {y:.2} Td (").as_bytes());
        for c in s.chars() {
            match win_ansi(c) {
                b @ (b'(' | b')' | b'\\') => self.ops.extend_from_slice(&[b'\\', b]),
                b => self.ops.push(b),
            }
        }
        self.ops.extend_from_slice(b") Tj ET\n");
    }
    fn centred(&mut self, s: &str, size: f32, page_w: f32, y: f32, bold: bool) {
        let x = (page_w - text_width(s, size, bold)) / 2.0;
        self.text(s, size, x.max(MARGIN / 2.0), y, bold);
    }
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, line: f32, grey: f32) {
        self.ops
            .extend_from_slice(format!("q {grey:.2} G {line:.2} w {x:.2} {y:.2} {w:.2} {h:.2} re S Q\n").as_bytes());
    }
    fn hline(&mut self, x1: f32, x2: f32, y: f32, line: f32) {
        self.ops.extend_from_slice(format!("q {line:.2} w {x1:.2} {y:.2} m {x2:.2} {y:.2} l S Q\n").as_bytes());
    }
    /// Draw image object `obj` (pixel size iw×ih) fitted and centred in the box.
    #[allow(clippy::too_many_arguments)]
    fn image_fit(&mut self, obj: usize, iw: u32, ih: u32, x: f32, y: f32, w: f32, h: f32) {
        let s = (w / iw as f32).min(h / ih as f32);
        let (dw, dh) = (iw as f32 * s, ih as f32 * s);
        let (dx, dy) = (x + (w - dw) / 2.0, y + (h - dh) / 2.0);
        self.ops.extend_from_slice(
            format!("q {dw:.2} 0 0 {dh:.2} {dx:.2} {dy:.2} cm /Im{obj} Do Q\n").as_bytes(),
        );
        if !self.images.contains(&obj) {
            self.images.push(obj);
        }
    }
}

fn add_image(pdf: &mut Pdf, img: &GrayImage) -> usize {
    let (w, h) = img.dimensions();
    let row = w.div_ceil(8) as usize;
    let mut bits = vec![0u8; row * h as usize];
    for (x, y, p) in img.enumerate_pixels() {
        if p[0] >= 128 {
            bits[y as usize * row + (x / 8) as usize] |= 0x80 >> (x % 8);
        }
    }
    let dict = format!(
        "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /FlateDecode"
    );
    pdf.add_stream(&dict, &deflate(&bits))
}

fn body_size(age: AgeBand) -> f32 {
    match age {
        AgeBand::Early => 26.0,
        AgeBand::Middle => 19.0,
        AgeBand::Older => 15.0,
    }
}

/// Largest font size (≤ start) at which `text` fits in `max_h`; returns (size, lines).
fn fit_text(text: &str, start: f32, max_w: f32, max_h: f32) -> (f32, Vec<String>) {
    let mut size = start;
    loop {
        let lines = wrap(text, size, max_w, false);
        if lines.len() as f32 * size * 1.45 <= max_h || size <= 9.0 {
            return (size, lines);
        }
        size -= 0.5;
    }
}

/// Build the whole book. `images` maps stored file names to processed pages.
pub fn build_book(project: &Project, images: &HashMap<String, GrayImage>) -> Vec<u8> {
    let (pw, ph) = project.paper.size_pt();
    let (cw, ch) = (pw - 2.0 * MARGIN, ph - 2.0 * MARGIN);

    let mut pdf = Pdf::new();
    let catalog = pdf.reserve();
    let pages_id = pdf.reserve();
    let f1 = pdf.add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec());
    let f2 =
        pdf.add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec());

    let mut image_objs: HashMap<String, (usize, u32, u32)> = HashMap::new();
    let mut image_obj = |pdf: &mut Pdf, name: &Option<String>| -> Option<(usize, u32, u32)> {
        let name = name.as_ref()?;
        if let Some(v) = image_objs.get(name) {
            return Some(*v);
        }
        let img = images.get(name)?;
        let v = (add_image(pdf, img), img.width(), img.height());
        image_objs.insert(name.clone(), v);
        Some(v)
    };

    let mut canvases: Vec<Canvas> = Vec::new();

    // Cover.
    {
        let mut c = Canvas::new();
        c.rect(MARGIN / 2.0, MARGIN / 2.0, pw - MARGIN, ph - MARGIN, 3.0, 0.0);
        let mut title_size: f32 = 40.0;
        let mut title_lines = wrap(&project.title, title_size, cw - 20.0, true);
        while title_lines.len() > 3 && title_size > 18.0 {
            title_size -= 2.0;
            title_lines = wrap(&project.title, title_size, cw - 20.0, true);
        }
        let mut y = ph - MARGIN - title_size - 10.0;
        for line in &title_lines {
            c.centred(line, title_size, pw, y, true);
            y -= title_size * 1.2;
        }
        let bottom = MARGIN + 70.0;
        if let Some((obj, iw, ih)) = image_obj(&mut pdf, &project.cover_image) {
            c.image_fit(obj, iw, ih, MARGIN, bottom, cw, y - bottom + title_size * 0.3);
        }
        if !project.author.trim().is_empty() {
            c.centred(&format!("by {}", project.author.trim()), 18.0, pw, MARGIN + 38.0, false);
        }
        c.centred(&format!("A read & colour storybook \u{2022} ages {}", project.age_band.label()), 11.0, pw, MARGIN + 14.0, false);
        canvases.push(c);
    }

    // "This book belongs to" page.
    {
        let mut c = Canvas::new();
        c.rect(MARGIN, MARGIN, cw, ch, 2.0, 0.0);
        c.centred("This book belongs to", 30.0, pw, ph * 0.62, true);
        c.hline(MARGIN + 60.0, pw - MARGIN - 60.0, ph * 0.5, 1.5);
        c.centred("Read each page, then colour the picture!", 14.0, pw, ph * 0.38, false);
        canvases.push(c);
    }

    let base = body_size(project.age_band);
    let mut number = 1;
    for page in &project.pages {
        let img = image_obj(&mut pdf, &page.image);
        match project.layout {
            Layout::FacingPages => {
                let mut t = Canvas::new();
                t.rect(MARGIN, MARGIN, cw, ch, 1.5, 0.35);
                let pad = 28.0;
                let (size, lines) = fit_text(&page.text, base, cw - 2.0 * pad, ch - 2.0 * pad - 20.0);
                let lead = size * 1.45;
                let block = lines.len() as f32 * lead;
                // Vertically centred, nudged slightly up.
                let mut y = MARGIN + ch / 2.0 + block / 2.0 - size + ch * 0.05;
                y = y.min(ph - MARGIN - pad - size);
                for line in &lines {
                    t.text(line, size, MARGIN + pad, y, false);
                    y -= lead;
                }
                t.centred(&number.to_string(), 10.0, pw, MARGIN / 2.0, false);
                canvases.push(t);
                number += 1;

                let mut p = Canvas::new();
                if let Some((obj, iw, ih)) = img {
                    p.image_fit(obj, iw, ih, MARGIN, MARGIN, cw, ch);
                }
                p.centred(&number.to_string(), 10.0, pw, MARGIN / 2.0, false);
                canvases.push(p);
                number += 1;
            }
            Layout::TextAbove => {
                let mut c = Canvas::new();
                let (size, lines) = fit_text(&page.text, base, cw - 10.0, ch * 0.38);
                let lead = size * 1.45;
                let mut y = ph - MARGIN - size;
                for line in &lines {
                    c.text(line, size, MARGIN + 5.0, y, false);
                    y -= lead;
                }
                let top = y + lead - size * 0.6;
                if let Some((obj, iw, ih)) = img {
                    c.image_fit(obj, iw, ih, MARGIN, MARGIN, cw, top - MARGIN);
                }
                c.centred(&number.to_string(), 10.0, pw, MARGIN / 2.0, false);
                canvases.push(c);
                number += 1;
            }
        }
    }

    // "The End".
    {
        let mut c = Canvas::new();
        c.centred("The End", 44.0, pw, ph * 0.55, true);
        canvases.push(c);
    }

    let mut kids = Vec::new();
    for c in canvases {
        let content = pdf.add_stream("/Filter /FlateDecode", &deflate(&c.ops));
        let xobjects: String = c.images.iter().map(|o| format!("/Im{o} {o} 0 R ")).collect();
        let page = pdf.add(
            format!(
                "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] \
                 /Resources << /Font << /F1 {f1} 0 R /F2 {f2} 0 R >> /XObject << {xobjects}>> >> \
                 /Contents {content} 0 R >>"
            )
            .into_bytes(),
        );
        kids.push(page);
    }
    let kid_refs: String = kids.iter().map(|k| format!("{k} 0 R ")).collect();
    pdf.set(pages_id, format!("<< /Type /Pages /Kids [{kid_refs}] /Count {} >>", kids.len()).into_bytes());
    pdf.set(catalog, format!("<< /Type /Catalog /Pages {pages_id} 0 R >>").into_bytes());
    pdf.finish(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Page;
    use image::Luma;

    fn sample(layout: Layout) -> (Project, HashMap<String, GrayImage>) {
        let mut p = Project::default();
        p.title = "Lulu's (Big) Day \u{2013} ê".into();
        p.layout = layout;
        p.pages = (0..3)
            .map(|i| Page {
                id: i.to_string(),
                text: format!("Page {i} text. ").repeat(20),
                image: Some("x.png".into()),
                ..Default::default()
            })
            .collect();
        let mut imgs = HashMap::new();
        imgs.insert("x.png".to_string(), GrayImage::from_pixel(30, 40, Luma([255])));
        (p, imgs)
    }

    #[test]
    fn xref_offsets_point_at_objects() {
        let (p, imgs) = sample(Layout::FacingPages);
        let pdf = build_book(&p, &imgs);
        assert!(pdf.starts_with(b"%PDF-1.4"));
        // Work on bytes: binary streams make a lossy UTF-8 view shift offsets.
        let key = b"startxref\n";
        let at = pdf.windows(key.len()).rposition(|w| w == key).unwrap() + key.len();
        let tail = std::str::from_utf8(&pdf[at..]).unwrap();
        let start: usize = tail.lines().next().unwrap().parse().unwrap();
        let xref = std::str::from_utf8(&pdf[start..]).unwrap();
        assert!(xref.starts_with("xref"));
        let entries: Vec<&str> = xref.lines().skip(3).take_while(|l| l.ends_with(" n ")).collect();
        assert!(entries.len() > 5);
        let text = String::from_utf8_lossy(&pdf);
        for (i, e) in entries.iter().enumerate() {
            let off: usize = e[..10].parse().unwrap();
            assert!(pdf[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()), "object {}", i + 1);
        }
        // cover + belongs-to + 3×2 + the end
        assert!(text.contains("/Count 9"));
        // the shared image is embedded once
        assert_eq!(text.matches("/Subtype /Image").count(), 1);
    }

    #[test]
    fn text_above_has_one_page_per_story_page() {
        let (p, imgs) = sample(Layout::TextAbove);
        let text = String::from_utf8_lossy(&build_book(&p, &imgs)).to_string();
        assert!(text.contains("/Count 6"));
    }

    #[test]
    fn wrap_respects_width() {
        let lines = wrap(&"word ".repeat(100), 20.0, 300.0, false);
        assert!(lines.len() > 5);
        assert!(lines.iter().all(|l| text_width(l, 20.0, false) <= 300.0));
    }

    #[test]
    fn encoding_and_escaping() {
        let mut c = Canvas::new();
        c.text("(ê) \u{201C}hi\u{201D}\\", 12.0, 0.0, 0.0, false);
        let s = c.ops;
        assert!(s.windows(4).any(|w| w == b"\\(\xEA\\"));
        assert!(s.windows(4).any(|w| w == b"\x93hi\x94"));
    }
}
