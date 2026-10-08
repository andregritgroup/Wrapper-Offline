use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Reading/colouring level. Drives words per page, font size, prompt style and line weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgeBand {
    /// Ages 5–7: very few words, very thick lines, big simple shapes.
    #[default]
    Early,
    /// Ages 8–10.
    Middle,
    /// Ages 11–12: longer text, finer lines, more detail.
    Older,
}

impl AgeBand {
    pub fn label(self) -> &'static str {
        match self {
            AgeBand::Early => "5\u{2013}7",
            AgeBand::Middle => "8\u{2013}10",
            AgeBand::Older => "11\u{2013}12",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// Story text on one page, full-page picture on the next.
    #[default]
    FacingPages,
    /// Story text at the top of the page, picture underneath.
    TextAbove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Paper {
    #[default]
    A4,
    Letter,
}

impl Paper {
    /// Width and height in PDF points.
    pub fn size_pt(self) -> (f32, f32) {
        match self {
            Paper::A4 => (595.28, 841.89),
            Paper::Letter => (612.0, 792.0),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Character {
    pub name: String,
    /// Comma-separated other names the story uses ("Grandpa, Oupa").
    pub aliases: String,
    /// Fixed visual description, repeated in every prompt for consistency.
    pub description: String,
}

impl Character {
    pub fn names(&self) -> Vec<String> {
        std::iter::once(self.name.as_str())
            .chain(self.aliases.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Page {
    /// Stable id so edits and generations never target the wrong page.
    pub id: String,
    pub text: String,
    /// Optional picture description; when empty the page text is illustrated.
    pub scene: String,
    /// Only ever set by the server (generate/upload), never by the client.
    pub image: Option<String>,
    pub attempt: u32,
    /// Derived: characters mentioned on this page.
    #[serde(skip_deserializing)]
    pub present: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub id: String,
    pub title: String,
    pub author: String,
    pub age_band: AgeBand,
    pub layout: Layout,
    pub paper: Paper,
    /// Extra art direction applied to every picture ("jungle setting, cute animals").
    pub style_notes: String,
    pub seed: u64,
    pub characters: Vec<Character>,
    pub story: String,
    pub pages: Vec<Page>,
    pub cover_scene: String,
    pub cover_image: Option<String>,
    pub cover_attempt: u32,
    pub updated_at: u64,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            id: String::new(),
            title: "My Colouring Storybook".into(),
            author: String::new(),
            age_band: AgeBand::default(),
            layout: Layout::default(),
            paper: Paper::default(),
            style_notes: String::new(),
            seed: now_nanos() % 1_000_000_007,
            characters: Vec::new(),
            story: String::new(),
            pages: Vec::new(),
            cover_scene: String::new(),
            cover_image: None,
            cover_attempt: 0,
            updated_at: 0,
        }
    }
}

impl Project {
    pub fn refresh_derived(&mut self) {
        for page in &mut self.pages {
            let haystack = format!("{}\n{}", page.text, page.scene);
            page.present = crate::story::characters_in(&self.characters, &haystack);
        }
    }

    pub fn page_index(&self, page_id: &str) -> Option<usize> {
        self.pages.iter().position(|p| p.id == page_id)
    }
}

/// Where a generated or uploaded picture goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    Cover,
    Page(String),
}

impl Slot {
    pub fn parse(s: &str) -> Slot {
        if s == "cover" {
            Slot::Cover
        } else {
            Slot::Page(s.to_string())
        }
    }
}

pub fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

pub fn now_secs() -> u64 {
    now_nanos() / 1_000_000_000
}

/// Short unique id, safe for file names and URLs.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:03x}", now_nanos() / 1000, n % 4096)
}
