//! Projects on disk: `<root>/projects/<id>/project.json` plus that book's PNG pages.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::ai::Settings;
use crate::model::{new_id, now_secs, Page, Project, Slot};

#[derive(Debug)]
pub struct NotFound(pub String);
impl fmt::Display for NotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} not found", self.0)
    }
}
impl std::error::Error for NotFound {}

#[derive(Serialize)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub pages: usize,
    pub illustrated: usize,
    pub updated_at: u64,
}

pub struct Store {
    root: PathBuf,
    lock: Mutex<()>,
}

fn safe_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') && !s.starts_with('.')
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Store> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("projects")).with_context(|| format!("creating {}", root.display()))?;
        Ok(Store { root, lock: Mutex::new(()) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, id: &str) -> Result<PathBuf> {
        if !safe_name(id) || id.contains('.') {
            return Err(NotFound(format!("book {id:?}")).into());
        }
        Ok(self.root.join("projects").join(id))
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn load_settings(&self) -> Result<Settings> {
        let path = self.root.join("settings.json");
        if !path.exists() {
            return Ok(Settings::default());
        }
        serde_json::from_slice(&fs::read(&path)?).context("reading settings.json")
    }

    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        write_atomic(&self.root.join("settings.json"), &serde_json::to_vec_pretty(s)?)
    }

    pub fn list(&self) -> Result<Vec<Summary>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(self.root.join("projects"))? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().to_string();
            if let Ok(p) = self.load(&id) {
                out.push(Summary {
                    id: p.id.clone(),
                    title: p.title.clone(),
                    pages: p.pages.len(),
                    illustrated: p.pages.iter().filter(|pg| pg.image.is_some()).count(),
                    updated_at: p.updated_at,
                });
            }
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        Ok(out)
    }

    pub fn load(&self, id: &str) -> Result<Project> {
        let path = self.dir(id)?.join("project.json");
        let bytes = fs::read(&path).map_err(|_| NotFound(format!("book {id:?}")))?;
        let mut p: Project = serde_json::from_slice(&bytes).with_context(|| format!("reading {}", path.display()))?;
        p.id = id.to_string();
        p.refresh_derived();
        Ok(p)
    }

    /// Writes the project and deletes page images it no longer references. Caller holds the lock.
    fn save_locked(&self, p: &mut Project) -> Result<()> {
        let dir = self.dir(&p.id)?;
        fs::create_dir_all(&dir)?;
        p.updated_at = now_secs();
        p.refresh_derived();
        write_atomic(&dir.join("project.json"), &serde_json::to_vec_pretty(p)?)?;
        let referenced: Vec<&str> = p
            .pages
            .iter()
            .filter_map(|pg| pg.image.as_deref())
            .chain(p.cover_image.as_deref())
            .chain(p.characters.iter().filter_map(|c| c.reference_image.as_deref()))
            .chain(p.style_image.as_deref())
            .collect();
        for entry in fs::read_dir(&dir)? {
            let name = entry?.file_name().to_string_lossy().to_string();
            if name.ends_with(".png") && !referenced.contains(&name.as_str()) {
                let _ = fs::remove_file(dir.join(&name));
            }
        }
        Ok(())
    }

    pub fn create(&self, title: Option<String>) -> Result<Project> {
        let _g = self.guard();
        let mut p = Project { id: new_id(), ..Default::default() };
        if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
            p.title = t.trim().to_string();
        }
        self.save_locked(&mut p)?;
        Ok(p)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let _g = self.guard();
        let dir = self.dir(id)?;
        if !dir.join("project.json").exists() {
            return Err(NotFound(format!("book {id:?}")).into());
        }
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// Apply the browser's edits. Pictures and attempt counters are server-owned and are
    /// carried over by page id, so a stale edit can never undo a fresh generation.
    pub fn update(&self, id: &str, incoming: Project) -> Result<Project> {
        let _g = self.guard();
        let disk = self.load(id)?;
        let mut p = incoming;
        p.id = disk.id.clone();
        p.cover_image = disk.cover_image.clone();
        p.cover_attempt = disk.cover_attempt;
        p.style_image = disk.style_image.clone();
        let mut seen_chars = std::collections::HashSet::new();
        for c in &mut p.characters {
            if c.id.is_empty() || !safe_name(&c.id) || !seen_chars.insert(c.id.clone()) {
                c.id = new_id();
                seen_chars.insert(c.id.clone());
            }
            match disk.characters.iter().find(|d| d.id == c.id) {
                Some(d) => {
                    c.reference_image = d.reference_image.clone();
                    c.ref_attempt = d.ref_attempt;
                }
                None => {
                    c.reference_image = None;
                    c.ref_attempt = 0;
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        for page in &mut p.pages {
            if page.id.is_empty() || !safe_name(&page.id) || !seen.insert(page.id.clone()) {
                page.id = new_id();
                seen.insert(page.id.clone());
            }
            match disk.pages.iter().find(|d| d.id == page.id) {
                Some(d) => {
                    page.image = d.image.clone();
                    page.attempt = d.attempt;
                }
                None => {
                    page.image = None;
                    page.attempt = 0;
                }
            }
        }
        self.save_locked(&mut p)?;
        Ok(p)
    }

    /// Replace all pages (after re-splitting the story) with (text, picture idea) pairs.
    pub fn replace_pages(&self, id: &str, pages: Vec<(String, String)>) -> Result<Project> {
        let _g = self.guard();
        let mut p = self.load(id)?;
        p.pages = pages
            .into_iter()
            .map(|(text, scene)| Page { id: new_id(), text, scene, ..Default::default() })
            .collect();
        self.save_locked(&mut p)?;
        Ok(p)
    }

    /// Store a processed PNG for the cover or a page.
    pub fn set_image(&self, id: &str, slot: &Slot, png: &[u8], from_ai: bool) -> Result<Project> {
        let _g = self.guard();
        let mut p = self.load(id)?;
        let dir = self.dir(id)?;
        let (prefix, target) = match slot {
            Slot::Cover => ("cover".to_string(), &mut p.cover_image),
            Slot::Page(pid) => {
                let idx = p.page_index(pid).ok_or_else(|| NotFound(format!("page {pid:?}")))?;
                let prefix = format!("page-{}", p.pages[idx].id);
                (prefix, &mut p.pages[idx].image)
            }
            Slot::Character(cid) => {
                let idx = p.character_index(cid).ok_or_else(|| NotFound(format!("character {cid:?}")))?;
                let prefix = format!("char-{}", p.characters[idx].id);
                (prefix, &mut p.characters[idx].reference_image)
            }
            Slot::Style => ("style".to_string(), &mut p.style_image),
        };
        let name = format!("{prefix}-{}.png", new_id());
        write_atomic(&dir.join(&name), png)?;
        *target = Some(name);
        if from_ai {
            match slot {
                Slot::Cover => p.cover_attempt += 1,
                Slot::Page(pid) => {
                    if let Some(i) = p.page_index(pid) {
                        p.pages[i].attempt += 1;
                    }
                }
                Slot::Character(cid) => {
                    if let Some(i) = p.character_index(cid) {
                        p.characters[i].ref_attempt += 1;
                    }
                }
                Slot::Style => {}
            }
        }
        self.save_locked(&mut p)?;
        Ok(p)
    }

    /// Copy a book with all its pictures, e.g. for a second edition with different text.
    pub fn duplicate(&self, id: &str) -> Result<Project> {
        let _g = self.guard();
        let mut p = self.load(id)?;
        let from = self.dir(id)?;
        p.id = new_id();
        p.title = format!("{} (copy)", p.title);
        let to = self.dir(&p.id)?;
        fs::create_dir_all(&to)?;
        for entry in fs::read_dir(&from)? {
            let name = entry?.file_name();
            if name.to_string_lossy().ends_with(".png") {
                fs::copy(from.join(&name), to.join(&name))?;
            }
        }
        self.save_locked(&mut p)?;
        Ok(p)
    }

    pub fn image_path(&self, id: &str, file: &str) -> Result<PathBuf> {
        if !safe_name(file) || !file.ends_with(".png") {
            return Err(NotFound(format!("image {file:?}")).into());
        }
        let path = self.dir(id)?.join(file);
        if !path.exists() {
            return Err(NotFound(format!("image {file:?}")).into());
        }
        Ok(path)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)?;
    Ok(())
}
