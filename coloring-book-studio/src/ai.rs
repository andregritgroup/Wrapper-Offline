//! Free image-generation back-ends.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use image::{GrayImage, Luma};
use imageproc::drawing::{draw_filled_circle_mut, draw_hollow_circle_mut, draw_line_segment_mut};
use serde::{Deserialize, Serialize};

use crate::story::ImagePrompt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// pollinations.ai – free, no account needed (rate limited).
    #[default]
    Pollinations,
    /// runware.ai – pay-as-you-go, about 1 US cent per picture with FLUX; the only option
    /// here that copies characters from their reference pictures.
    Runware,
    /// Hugging Face Inference Providers – free account token, small monthly free quota.
    HuggingFace,
    /// Your own Stable Diffusion (AUTOMATIC1111 / Forge / SD.Next) – free and unlimited, needs a GPU.
    LocalSd,
    /// Offline test drawings, no AI. Useful to try the app or lay out a book.
    Placeholder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: Provider,
    pub pollinations_url: String,
    pub pollinations_model: String,
    pub pollinations_token: String,
    pub hf_token: String,
    pub hf_model: String,
    pub sd_url: String,
    pub sd_steps: u32,
    /// Appended to every prompt for local SD, e.g. a line-art LoRA tag.
    pub sd_extra_prompt: String,
    pub runware_token: String,
    /// Model for pictures without a character reference (FLUX.1 [dev]).
    pub runware_model: String,
    /// Model used when a character reference picture is supplied (FLUX.1 Kontext [dev]).
    pub runware_ref_model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            provider: Provider::Pollinations,
            pollinations_url: "https://image.pollinations.ai".into(),
            pollinations_model: "flux".into(),
            pollinations_token: String::new(),
            hf_token: String::new(),
            hf_model: "black-forest-labs/FLUX.1-schnell".into(),
            sd_url: "http://127.0.0.1:7860".into(),
            sd_steps: 28,
            sd_extra_prompt: "line art, coloring book, lineart".into(),
            runware_token: String::new(),
            runware_model: "runware:101@1".into(),
            runware_ref_model: "runware:106@1".into(),
        }
    }
}

/// What the browser sees: secrets replaced by "is it set?" flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicSettings {
    #[serde(flatten)]
    pub settings: Settings,
    pub pollinations_token_set: bool,
    pub hf_token_set: bool,
    pub runware_token_set: bool,
}

impl Settings {
    pub fn public(&self) -> PublicSettings {
        let mut s = self.clone();
        s.pollinations_token.clear();
        s.hf_token.clear();
        s.runware_token.clear();
        PublicSettings {
            settings: s,
            pollinations_token_set: !self.pollinations_token.is_empty(),
            hf_token_set: !self.hf_token.is_empty(),
            runware_token_set: !self.runware_token.is_empty(),
        }
    }

    /// Apply an update from the browser; empty secrets mean "keep the current one".
    pub fn merge(&mut self, mut incoming: Settings) {
        if incoming.pollinations_token.trim().is_empty() {
            incoming.pollinations_token = std::mem::take(&mut self.pollinations_token);
        }
        if incoming.hf_token.trim().is_empty() {
            incoming.hf_token = std::mem::take(&mut self.hf_token);
        }
        if incoming.runware_token.trim().is_empty() {
            incoming.runware_token = std::mem::take(&mut self.runware_token);
        }
        incoming.runware_token = incoming.runware_token.trim().to_string();
        incoming.pollinations_token = incoming.pollinations_token.trim().to_string();
        incoming.hf_token = incoming.hf_token.trim().to_string();
        *self = incoming;
    }
}

impl Provider {
    /// Whether this provider uses character reference pictures.
    pub fn uses_references(self) -> bool {
        self == Provider::Runware
    }
}

/// Generate one image; returns encoded image bytes (PNG/JPEG/WebP).
/// `reference` is a PNG sheet of the characters to copy (only used by providers that support it).
pub async fn generate(
    http: &reqwest::Client,
    s: &Settings,
    req: &ImagePrompt,
    reference: Option<Vec<u8>>,
) -> Result<Vec<u8>> {
    match s.provider {
        Provider::Pollinations => pollinations(http, s, req).await,
        Provider::Runware => runware(http, s, req, reference).await,
        Provider::HuggingFace => hugging_face(http, s, req).await,
        Provider::LocalSd => local_sd(http, s, req).await,
        Provider::Placeholder => placeholder(req),
    }
}

fn check_image(bytes: &[u8], who: &str) -> Result<()> {
    image::guess_format(bytes)
        .map(|_| ())
        .map_err(|_| anyhow!("{who} did not return an image: {}", snippet(bytes)))
}

fn snippet(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).trim().to_string()
}

fn retryable(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

async fn pollinations(http: &reqwest::Client, s: &Settings, r: &ImagePrompt) -> Result<Vec<u8>> {
    let url = format!(
        "{}/prompt/{}",
        s.pollinations_url.trim_end_matches('/'),
        urlencoding::encode(&r.prompt)
    );
    let query = [
        ("width", r.width.to_string()),
        ("height", r.height.to_string()),
        ("seed", r.seed.to_string()),
        ("model", s.pollinations_model.clone()),
        ("nologo", "true".into()),
        ("private", "true".into()),
        ("enhance", "false".into()),
        ("safe", "true".into()),
    ];
    let mut last = String::new();
    for attempt in 0..4u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(8 * 2u64.pow(attempt - 1))).await;
        }
        let mut rb = http.get(&url).query(&query);
        if !s.pollinations_token.is_empty() {
            rb = rb.bearer_auth(&s.pollinations_token);
        }
        let resp = rb.send().await.context("could not reach Pollinations")?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if status.is_success() {
            check_image(&bytes, "Pollinations")?;
            return Ok(bytes.to_vec());
        }
        last = format!("Pollinations returned {status}: {}", snippet(&bytes));
        if !retryable(status) {
            break;
        }
    }
    bail!(last)
}

async fn runware(http: &reqwest::Client, s: &Settings, r: &ImagePrompt, reference: Option<Vec<u8>>) -> Result<Vec<u8>> {
    if s.runware_token.is_empty() {
        bail!("Runware needs an API key (Settings tab, or RUNWARE_API_KEY environment variable)");
    }
    let mut task = serde_json::json!({
        "taskType": "imageInference",
        "taskUUID": uuid_v4(),
        "positivePrompt": r.prompt,
        "width": r.width,
        "height": r.height,
        "seed": r.seed.max(1),
        "numberResults": 1,
        "outputType": "base64Data",
        "outputFormat": "PNG",
    });
    match reference {
        Some(png) => {
            task["model"] = s.runware_ref_model.trim().into();
            task["positivePrompt"] = format!(
                "Draw a completely new picture using the character(s) shown in the reference image, keeping \
                 their faces, bodies, proportions and clothes exactly the same. Do not copy the reference \
                 layout. {}",
                r.prompt
            )
            .into();
            let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
            task["inputs"] = serde_json::json!({ "referenceImages": [uri] });
        }
        None => task["model"] = s.runware_model.trim().into(),
    }

    let mut last = String::new();
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(5 * attempt as u64)).await;
        }
        let resp = http
            .post(std::env::var("RUNWARE_URL").unwrap_or_else(|_| "https://api.runware.ai/v1".into()))
            .bearer_auth(&s.runware_token)
            .json(&serde_json::json!([task]))
            .send()
            .await
            .context("could not reach Runware")?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        if let Some(msg) = body["errors"][0]["message"].as_str() {
            last = format!("Runware: {msg}");
        } else if status.is_success() {
            let item = &body["data"][0];
            if let Some(b64) = item["imageBase64Data"].as_str() {
                let img = base64::engine::general_purpose::STANDARD.decode(b64.trim())?;
                check_image(&img, "Runware")?;
                return Ok(img);
            }
            if let Some(url) = item["imageURL"].as_str() {
                let img = http.get(url).send().await?.error_for_status()?.bytes().await?.to_vec();
                check_image(&img, "Runware")?;
                return Ok(img);
            }
            last = format!("Runware returned no image: {}", snippet(&bytes));
        } else {
            last = format!("Runware returned {status}: {}", snippet(&bytes));
        }
        if !retryable(status) {
            break;
        }
    }
    bail!(last)
}

fn uuid_v4() -> String {
    // Version-4 UUID from a time-seeded xorshift; uniqueness per request is all Runware needs.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut x = crate::model::now_nanos() ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ ((std::process::id() as u64) << 32);
    let mut b = [0u8; 16];
    for chunk in b.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        chunk.copy_from_slice(&x.to_le_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|v| format!("{v:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

async fn hugging_face(http: &reqwest::Client, s: &Settings, r: &ImagePrompt) -> Result<Vec<u8>> {
    if s.hf_token.is_empty() {
        bail!("Hugging Face needs a free access token (Settings tab, or HF_TOKEN environment variable)");
    }
    let url = format!(
        "https://router.huggingface.co/hf-inference/models/{}",
        s.hf_model.trim().trim_matches('/')
    );
    let mut params = serde_json::json!({ "width": r.width, "height": r.height, "seed": r.seed });
    // FLUX models ignore/reject negative prompts; SD-family models use them.
    if !s.hf_model.to_lowercase().contains("flux") {
        params["negative_prompt"] = r.negative.clone().into();
    }
    let body = serde_json::json!({ "inputs": r.prompt, "parameters": params });
    let mut last = String::new();
    for attempt in 0..4u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(10 * attempt as u64)).await;
        }
        let resp = http
            .post(&url)
            .bearer_auth(&s.hf_token)
            .header("Accept", "image/png")
            .json(&body)
            .send()
            .await
            .context("could not reach Hugging Face")?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if status.is_success() {
            check_image(&bytes, "Hugging Face")?;
            return Ok(bytes.to_vec());
        }
        last = format!("Hugging Face returned {status}: {}", snippet(&bytes));
        if status == reqwest::StatusCode::PAYMENT_REQUIRED {
            last.push_str(" (free monthly credits used up – switch provider or wait for the reset)");
        }
        if !retryable(status) {
            break;
        }
    }
    bail!(last)
}

async fn local_sd(http: &reqwest::Client, s: &Settings, r: &ImagePrompt) -> Result<Vec<u8>> {
    let url = format!("{}/sdapi/v1/txt2img", s.sd_url.trim_end_matches('/'));
    let prompt = if s.sd_extra_prompt.trim().is_empty() {
        r.prompt.clone()
    } else {
        format!("{}, {}", s.sd_extra_prompt.trim(), r.prompt)
    };
    let body = serde_json::json!({
        "prompt": prompt,
        "negative_prompt": r.negative,
        "width": r.width,
        "height": r.height,
        "seed": r.seed,
        "steps": s.sd_steps.clamp(1, 150),
        "cfg_scale": 7,
        "batch_size": 1,
    });
    let resp = http
        .post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("could not reach Stable Diffusion at {} – is it running with --api?", s.sd_url))?;
    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() {
        bail!("Stable Diffusion returned {status}: {}", snippet(&bytes));
    }
    #[derive(Deserialize)]
    struct Out {
        images: Vec<String>,
    }
    let out: Out = serde_json::from_slice(&bytes).context("unexpected Stable Diffusion response")?;
    let first = out.images.into_iter().next().ok_or_else(|| anyhow!("Stable Diffusion returned no images"))?;
    let b64 = first.split_once(',').map(|(_, b)| b.to_string()).unwrap_or(first);
    let png = base64::engine::general_purpose::STANDARD.decode(b64.trim())?;
    check_image(&png, "Stable Diffusion")?;
    Ok(png)
}

/// A simple deterministic doodle so the whole pipeline can be tried offline.
fn placeholder(r: &ImagePrompt) -> Result<Vec<u8>> {
    let (w, h) = (r.width, r.height);
    let mut img = GrayImage::from_pixel(w, h, Luma([255]));
    let ink = Luma([0u8]);
    let mut rng = r.seed.max(1);
    let mut next = |max: u32| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng % max as u64) as i32
    };
    let thick_circle = |img: &mut GrayImage, cx: i32, cy: i32, rad: i32| {
        for d in 0..4 {
            draw_hollow_circle_mut(img, (cx, cy), rad - d, ink);
        }
    };
    let thick_line = |img: &mut GrayImage, a: (f32, f32), b: (f32, f32)| {
        for d in -2..=2 {
            let o = d as f32;
            draw_line_segment_mut(img, (a.0 + o, a.1), (b.0 + o, b.1), ink);
            draw_line_segment_mut(img, (a.0, a.1 + o), (b.0, b.1 + o), ink);
        }
    };
    let (wf, hf) = (w as f32, h as f32);
    // Ground and hills.
    thick_line(&mut img, (0.0, hf * 0.8), (wf, hf * 0.8));
    thick_circle(&mut img, (w / 5) as i32, (h as f32 * 0.95) as i32, (w / 4) as i32);
    // Sun.
    let (sx, sy) = (w as i32 - 110, 110);
    thick_circle(&mut img, sx, sy, 55);
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::PI / 4.0;
        thick_line(
            &mut img,
            (sx as f32 + 70.0 * a.cos(), sy as f32 + 70.0 * a.sin()),
            (sx as f32 + 100.0 * a.cos(), sy as f32 + 100.0 * a.sin()),
        );
    }
    // A friendly character: head, eyes, smile, body.
    let cx = (w / 2) as i32 + next(120) - 60;
    let cy = (h as f32 * 0.42) as i32;
    thick_circle(&mut img, cx, cy, 120);
    thick_circle(&mut img, cx - 60, cy - 105, 40);
    thick_circle(&mut img, cx + 60, cy - 105, 40);
    draw_filled_circle_mut(&mut img, (cx - 40, cy - 20), 14, ink);
    draw_filled_circle_mut(&mut img, (cx + 40, cy - 20), 14, ink);
    thick_line(&mut img, ((cx - 45) as f32, (cy + 45) as f32), (cx as f32, (cy + 70) as f32));
    thick_line(&mut img, (cx as f32, (cy + 70) as f32), ((cx + 45) as f32, (cy + 45) as f32));
    let body_top = (cy + 120) as f32;
    thick_line(&mut img, (cx as f32 - 90.0, hf * 0.8), (cx as f32 - 40.0, body_top));
    thick_line(&mut img, (cx as f32 + 90.0, hf * 0.8), (cx as f32 + 40.0, body_top));
    // Flowers.
    for _ in 0..3 {
        let fx = 60 + next(w - 120);
        let fy = (hf * 0.88) as i32 + next(60);
        thick_circle(&mut img, fx, fy, 18);
        for k in 0..5 {
            let a = k as f32 * 1.2566;
            thick_circle(&mut img, fx + (32.0 * a.cos()) as i32, fy + (32.0 * a.sin()) as i32, 14);
        }
    }
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}
