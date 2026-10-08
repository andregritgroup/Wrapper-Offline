//! Splitting a story into pages and turning a page into an image prompt.

use crate::model::{AgeBand, Character, Layout, Project, Slot};

/// Target words per page for each age band.
pub fn word_budget(age: AgeBand) -> usize {
    match age {
        AgeBand::Early => 50,
        AgeBand::Middle => 100,
        AgeBand::Older => 160,
    }
}

fn is_break_line(line: &str) -> bool {
    let t = line.trim().to_lowercase();
    matches!(t.as_str(), "---" | "***" | "[page]" | "[page break]" | "[pagebreak]")
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

struct Unit {
    text: String,
    para_start: bool,
    words: usize,
}

/// Split a story into page texts.
///
/// * Explicit breaks (a line containing only `---`, `***` or `[page]`) always win.
/// * Otherwise paragraphs are packed into pages of roughly [`word_budget`] words,
///   splitting long paragraphs at sentence boundaries.
/// * With `target_pages`, the text is distributed as evenly as possible over that
///   many pages (fewer if the story has fewer sentences than pages).
pub fn split_story(story: &str, age: AgeBand, target_pages: Option<usize>) -> Vec<String> {
    let story = story.replace("\r\n", "\n").replace('\r', "\n");

    if story.lines().any(is_break_line) {
        let mut pages = Vec::new();
        let mut current = String::new();
        for line in story.lines() {
            if is_break_line(line) {
                push_nonempty(&mut pages, &current);
                current.clear();
            } else {
                current.push_str(line);
                current.push('\n');
            }
        }
        push_nonempty(&mut pages, &current);
        return pages;
    }

    let paragraphs = paragraphs(&story);
    let total: usize = paragraphs.iter().map(|p| word_count(p)).sum();
    if total == 0 {
        return Vec::new();
    }
    let target = target_pages.filter(|&n| n > 0);
    let budget = match target {
        Some(n) => total.div_ceil(n).max(1),
        None => word_budget(age),
    };

    let mut units = Vec::new();
    for para in &paragraphs {
        let words = word_count(para);
        if target.is_some() || words > budget {
            for (i, s) in sentences(para).into_iter().enumerate() {
                let words = word_count(&s);
                units.push(Unit { text: s, para_start: i == 0, words });
            }
        } else {
            units.push(Unit { text: para.clone(), para_start: true, words });
        }
    }

    let groups: Vec<Vec<&Unit>> = match target {
        Some(n) => {
            let mut groups: Vec<Vec<&Unit>> = (0..n).map(|_| Vec::new()).collect();
            let mut before = 0usize;
            for u in &units {
                // Place each unit by where its midpoint falls in the story.
                let mid2 = before * 2 + u.words; // 2 × midpoint, avoids fractions
                let page = ((mid2 * n) / (total * 2)).min(n - 1);
                groups[page].push(u);
                before += u.words;
            }
            groups
        }
        None => {
            let mut groups: Vec<Vec<&Unit>> = Vec::new();
            let mut current: Vec<&Unit> = Vec::new();
            let mut words = 0;
            for u in &units {
                let over = words + u.words > budget + budget / 5;
                if !current.is_empty() && over && words >= budget / 2 {
                    groups.push(std::mem::take(&mut current));
                    words = 0;
                }
                words += u.words;
                current.push(u);
            }
            if !current.is_empty() {
                // Fold a tiny last page into the previous one.
                let tail: usize = current.iter().map(|u| u.words).sum();
                match groups.last_mut() {
                    Some(prev)
                        if tail < budget / 4
                            && prev.iter().map(|u| u.words).sum::<usize>() + tail
                                <= budget * 8 / 5 =>
                    {
                        prev.extend(current)
                    }
                    _ => groups.push(current),
                }
            }
            groups
        }
    };

    groups
        .into_iter()
        .filter(|g| !g.is_empty())
        .map(|g| {
            let mut text = String::new();
            for u in g {
                if !text.is_empty() {
                    text.push_str(if u.para_start { "\n\n" } else { " " });
                }
                text.push_str(&u.text);
            }
            text
        })
        .collect()
}

fn push_nonempty(pages: &mut Vec<String>, text: &str) {
    let t = text.trim();
    if !t.is_empty() {
        pages.push(t.to_string());
    }
}

/// Paragraphs separated by blank lines; single line breaks inside are kept (poems, dialogue).
fn paragraphs(story: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in story.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(current.join("\n"));
                current.clear();
            }
        } else {
            current.push(line.trim_end());
        }
    }
    if !current.is_empty() {
        out.push(current.join("\n"));
    }
    out
}

/// Split at `.`, `!` or `?` (plus closing quotes/brackets) followed by whitespace.
pub fn sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        if matches!(chars[i], '.' | '!' | '?') {
            let mut j = i + 1;
            while j < chars.len()
                && matches!(chars[j], '.' | '!' | '?' | '"' | '\'' | '\u{201D}' | '\u{2019}' | ')')
            {
                j += 1;
            }
            if j == chars.len() || chars[j].is_whitespace() {
                let s: String = chars[start..j].iter().collect();
                if !s.trim().is_empty() {
                    out.push(s.trim().to_string());
                }
                start = j;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    let rest: String = chars[start..].iter().collect();
    if !rest.trim().is_empty() {
        out.push(rest.trim().to_string());
    }
    out
}

/// Whole-word, case-insensitive match ("Sam" matches "Sam's" but not "Samantha").
fn contains_word(haystack_lower: &str, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    if needle.is_empty() {
        return false;
    }
    haystack_lower.match_indices(&needle).any(|(i, m)| {
        let before = haystack_lower[..i].chars().next_back();
        let after = haystack_lower[i + m.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Names of the characters mentioned (by name or alias) in `text`.
pub fn characters_in(characters: &[Character], text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    characters
        .iter()
        .filter(|c| !c.name.trim().is_empty())
        .filter(|c| c.names().iter().any(|n| contains_word(&lower, n)))
        .map(|c| c.name.trim().to_string())
        .collect()
}

fn truncate_words(s: &str, max: usize) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() <= max {
        words.join(" ")
    } else {
        format!("{}\u{2026}", words[..max].join(" "))
    }
}

pub struct ImagePrompt {
    pub prompt: String,
    pub negative: String,
    pub width: u32,
    pub height: u32,
    pub seed: u64,
    /// Ids of characters whose reference pictures apply to this picture.
    pub references: Vec<String>,
}

/// More characters than this in one reference sheet makes each one too small to copy reliably.
pub const MAX_REFERENCES: usize = 3;

const NEGATIVE: &str = "color, colour, colored, shading, shadows, gradient, grey fill, gray tones, \
halftone, crosshatching, solid black areas, photo, photorealistic, 3d render, text, letters, words, \
title, watermark, signature, frame clutter, blurry, sketchy, messy lines, scary, violent, blood, weapon";

fn age_style(age: AgeBand) -> &'static str {
    match age {
        AgeBand::Early => {
            "very simple coloring page for young children aged 5 to 7, extra thick bold outlines, \
             big simple rounded shapes, large open areas to colour, very little background detail, \
             cute friendly cartoon style"
        }
        AgeBand::Middle => {
            "coloring page for children aged 8 to 10, bold clean outlines, medium detail, \
             simple background scenery, friendly cartoon storybook style"
        }
        AgeBand::Older => {
            "detailed coloring page for children aged 11 to 12, clean confident outlines, \
             more intricate details and a fuller background with patterns, storybook illustration style"
        }
    }
}

/// Build the image prompt for a cover or page. `None` if the page id is unknown.
pub fn image_prompt(project: &Project, slot: &Slot) -> Option<ImagePrompt> {
    let (scene, present, index, attempt, portrait) = match slot {
        Slot::Cover => {
            let chars: Vec<String> = project
                .characters
                .iter()
                .filter(|c| !c.name.trim().is_empty())
                .take(4)
                .map(|c| c.name.trim().to_string())
                .collect();
            let scene = if project.cover_scene.trim().is_empty() {
                if chars.is_empty() {
                    format!("A cheerful storybook cover scene for a story called \"{}\"", project.title)
                } else {
                    format!(
                        "A cheerful storybook cover illustration showing {} together, smiling",
                        chars.join(", ")
                    )
                }
            } else {
                project.cover_scene.clone()
            };
            (scene, chars, 0u64, project.cover_attempt, true)
        }
        Slot::Page(id) => {
            let idx = project.page_index(id)?;
            let page = &project.pages[idx];
            let scene = if page.scene.trim().is_empty() {
                format!("Illustrate this moment from the story: {}", page.text)
            } else {
                page.scene.clone()
            };
            let present = characters_in(&project.characters, &format!("{}\n{}", page.text, page.scene));
            (scene, present, idx as u64 + 1, page.attempt, project.layout == Layout::FacingPages)
        }
        Slot::Character(id) => {
            let idx = project.character_index(id)?;
            let c = &project.characters[idx];
            let scene = format!(
                "Character design reference for {}: only this one character, full body from head to feet, \
                 standing and facing forward, friendly expression, centred, nothing else in the picture",
                c.name.trim()
            );
            (scene, vec![c.name.trim().to_string()], 100_000 + idx as u64, c.ref_attempt, true)
        }
    };

    let mut prompt = format!(
        "Black and white children's coloring book page. {}. Pure white background, crisp black ink \
         outlines only, no shading, no grey, no colour, no solid black fills, every shape fully closed \
         so it can be coloured in, no text or letters anywhere. Scene: {}.",
        age_style(project.age_band),
        truncate_words(&scene, 110).trim_end_matches(['.', ' '])
    );
    let descriptions: Vec<String> = project
        .characters
        .iter()
        .filter(|c| present.iter().any(|p| p == c.name.trim()))
        .filter(|c| !c.description.trim().is_empty())
        .map(|c| format!("{}: {}", c.name.trim(), truncate_words(&c.description, 45)))
        .collect();
    if !descriptions.is_empty() {
        prompt.push_str(&format!(" Characters (draw them exactly like this): {}.", descriptions.join("; ")));
    }
    if !project.style_notes.trim().is_empty() {
        prompt.push_str(&format!(" Style: {}.", truncate_words(&project.style_notes, 40)));
    }

    let (width, height) = if portrait { (768, 1024) } else { (1024, 1024) };
    let seed = (project.seed + index * 1009 + attempt as u64 * 7919) % 2_147_483_647;
    // Reference pictures the image model should copy the characters' look from.
    let references = match slot {
        Slot::Character(_) => Vec::new(),
        _ => project
            .characters
            .iter()
            .filter(|c| c.reference_image.is_some() && present.iter().any(|p| p == c.name.trim()))
            .take(MAX_REFERENCES)
            .map(|c| c.id.clone())
            .collect(),
    };
    Some(ImagePrompt { prompt, negative: NEGATIVE.into(), width, height, seed, references })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Page;

    #[test]
    fn explicit_breaks_win() {
        let s = "Once upon a time.\n---\nThe end came.\n\n[page]\n\nReally the end.";
        let pages = split_story(s, AgeBand::Older, Some(1));
        assert_eq!(pages, vec!["Once upon a time.", "The end came.", "Really the end."]);
    }

    #[test]
    fn auto_split_respects_budget_and_keeps_all_words() {
        let para = "The fox ran far. ".repeat(10); // 40 words
        let story = vec![para.trim(); 6].join("\n\n"); // 240 words
        let pages = split_story(&story, AgeBand::Early, None);
        assert!(pages.len() >= 4, "{} pages", pages.len());
        for p in &pages {
            assert!(word_count(p) <= 50 + 10, "page too long: {}", word_count(p));
        }
        let total: usize = pages.iter().map(|p| word_count(p)).sum();
        assert_eq!(total, 240);
    }

    #[test]
    fn long_paragraph_is_split_at_sentences() {
        let story = "Mia woke up early. ".repeat(30); // 120 words, one paragraph
        let pages = split_story(&story, AgeBand::Early, None);
        assert!(pages.len() >= 2);
        assert!(pages.iter().all(|p| p.ends_with('.')));
    }

    #[test]
    fn target_pages_gives_exact_count() {
        let story = (1..=40).map(|i| format!("Sentence number {i} is here.")).collect::<Vec<_>>().join(" ");
        for n in [1, 3, 7, 20, 40] {
            assert_eq!(split_story(&story, AgeBand::Middle, Some(n)).len(), n, "n={n}");
        }
        // Can't make more pages than sentences.
        assert_eq!(split_story(&story, AgeBand::Middle, Some(60)).len(), 40);
    }

    #[test]
    fn sentence_splitting_handles_quotes() {
        let s = sentences("\"Hello!\" said Tom. Is it Mr. Fox? Yes.");
        assert_eq!(s, vec!["\"Hello!\"", "said Tom.", "Is it Mr.", "Fox?", "Yes."]);
    }

    #[test]
    fn character_detection_is_whole_word() {
        let chars = vec![
            Character { name: "Sam".into(), ..Default::default() },
            Character { name: "Grandma Rose".into(), aliases: "Ouma, Gran".into(), ..Default::default() },
        ];
        assert_eq!(characters_in(&chars, "Samantha went home."), Vec::<String>::new());
        assert_eq!(characters_in(&chars, "It was Sam's kite."), vec!["Sam"]);
        assert_eq!(characters_in(&chars, "OUMA laughed."), vec!["Grandma Rose"]);
    }

    #[test]
    fn prompt_includes_present_character_descriptions_only() {
        let mut p = Project::default();
        p.characters = vec![
            Character { id: "l".into(), name: "Lulu".into(), description: "a small giraffe with a scarf".into(), ..Default::default() },
            Character { id: "b".into(), name: "Bo".into(), description: "a round bear".into(), reference_image: Some("b.png".into()), ..Default::default() },
        ];
        p.pages = vec![Page { id: "a".into(), text: "Lulu found a shell.".into(), ..Default::default() }];
        let ip = image_prompt(&p, &Slot::Page("a".into())).unwrap();
        assert!(ip.prompt.contains("small giraffe"));
        assert!(!ip.prompt.contains("round bear"));
        assert!(image_prompt(&p, &Slot::Page("missing".into())).is_none());
        assert!(ip.references.is_empty(), "Lulu has no reference picture");
        let cover = image_prompt(&p, &Slot::Cover).unwrap();
        assert!(cover.prompt.contains("round bear") && cover.prompt.contains("small giraffe"));
        assert_eq!(cover.references, vec!["b"]);
        let sheet = image_prompt(&p, &Slot::Character("l".into())).unwrap();
        assert!(sheet.prompt.contains("small giraffe") && !sheet.prompt.contains("round bear"));
        assert!(sheet.references.is_empty());
    }

    #[test]
    fn regenerating_changes_seed() {
        let mut p = Project::default();
        p.pages = vec![Page { id: "a".into(), text: "x".into(), ..Default::default() }];
        let s1 = image_prompt(&p, &Slot::Page("a".into())).unwrap().seed;
        p.pages[0].attempt += 1;
        let s2 = image_prompt(&p, &Slot::Page("a".into())).unwrap().seed;
        assert_ne!(s1, s2);
    }
}
