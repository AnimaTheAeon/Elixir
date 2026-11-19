//! Builds file and folder names from Midgar/Turks-style formats such as
//! "{Series Title} - S{season:00}E{episode:00} - {Episode Title}".

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// How Midgar names episodes. Read from Midgar when it's connected.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EpisodeNaming {
    pub standard: String,
    pub daily: String,
    pub anime: String,
    pub series_folder: String,
    pub season_folder: String,
    pub specials_folder: String,
    /// Sonarr's multi-episode style: 0 Extend, 1 Duplicate, 2 Repeat, 3 Scene, 4 Range, 5 Prefixed Range.
    pub multi_style: u8,
    /// Sonarr's colon replacement: 0 Delete, 1 Dash, 2 Space Dash, 3 Space Dash Space, 4 Smart.
    pub colon: u8,
}

impl Default for EpisodeNaming {
    fn default() -> Self {
        Self {
            standard: "{Series Title} - S{season:00}E{episode:00} - {Episode Title}".into(),
            daily: "{Series Title} - {Air-Date} - {Episode Title}".into(),
            anime: "{Series Title} - S{season:00}E{episode:00} - {Episode Title}".into(),
            series_folder: "{Series Title}".into(),
            season_folder: "Season {season}".into(),
            specials_folder: "Specials".into(),
            multi_style: 5,
            colon: 4,
        }
    }
}

/// How Turks names movies.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MovieNaming {
    pub file: String,
    pub folder: String,
    pub colon: u8,
}

impl Default for MovieNaming {
    fn default() -> Self {
        Self { file: "{Movie Title} ({Release Year})".into(), folder: "{Movie Title} ({Release Year})".into(), colon: 4 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Values {
    pub series_title: String,
    pub series_year: Option<u16>,
    pub season: Option<u32>,
    pub episodes: Vec<u32>,
    pub absolute: Vec<u32>,
    pub episode_titles: Vec<String>,
    pub air_date: Option<String>,
    pub movie_title: String,
    pub movie_year: Option<u16>,
}

fn pad(n: u32, spec: &str) -> String {
    let width = spec.chars().filter(|c| *c == '0').count().max(1);
    format!("{n:0width$}")
}

/// Fill one token. `sep` is the separator used in the token name (" ", ".", "_").
fn token(name: &str, spec: &str, v: &Values, multi: u8) -> Option<String> {
    let n = name.to_ascii_lowercase().replace(['.', '_', '-'], " ");
    let year_suffix = |t: &str, y: Option<u16>| match y {
        Some(y) if !t.contains(&format!("({y})")) => format!("{t} ({y})"),
        _ => t.to_string(),
    };
    Some(match n.as_str() {
        "series title" | "series titlethe" => v.series_title.clone(),
        "series titleyear" => year_suffix(&v.series_title, v.series_year),
        "series cleantitle" => clean_title(&v.series_title),
        "series cleantitleyear" => clean_title(&year_suffix(&v.series_title, v.series_year)),
        "season" => pad(v.season.unwrap_or(0), spec),
        "episode" => {
            let first = *v.episodes.first()?;
            let last = *v.episodes.last()?;
            if v.episodes.len() < 2 {
                pad(first, spec)
            } else {
                match multi {
                    // Prefixed range: 01-E03 (the "E" before the first number is in the format).
                    5 => format!("{}-E{}", pad(first, spec), pad(last, spec)),
                    4 => format!("{}-{}", pad(first, spec), pad(last, spec)),
                    _ => v.episodes.iter().map(|e| pad(*e, spec)).collect::<Vec<_>>().join("-E"),
                }
            }
        }
        "absolute" => v.absolute.iter().map(|e| pad(*e, spec)).collect::<Vec<_>>().join("-"),
        "episode title" | "episode cleantitle" => {
            let mut titles: Vec<String> = Vec::new();
            for t in &v.episode_titles {
                // "Part 1"/"Part 2" style pairs collapse to one title.
                let base = t.trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ' || c == '(' || c == ')').trim_end_matches("Part").trim();
                if !titles.iter().any(|x| x == t || (!base.is_empty() && x.starts_with(base))) {
                    titles.push(t.clone());
                }
            }
            let joined = titles.join(" + ");
            if n == "episode cleantitle" { clean_title(&joined) } else { joined }
        }
        "air date" => v.air_date.clone().unwrap_or_default().replace('-', " "),
        "air date hyphen" | "airdate" => v.air_date.clone().unwrap_or_default(),
        "movie title" | "movie titlethe" => v.movie_title.clone(),
        "movie titleyear" => year_suffix(&v.movie_title, v.movie_year),
        "movie cleantitle" => clean_title(&v.movie_title),
        "movie cleantitleyear" => clean_title(&year_suffix(&v.movie_title, v.movie_year)),
        "release year" | "year" => v.movie_year.or(v.series_year).map(|y| y.to_string()).unwrap_or_default(),
        // Things Elixir doesn't know (quality, release group, ...) are left out.
        _ => String::new(),
    })
}

fn clean_title(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Fill a format. Separators left dangling by empty tokens are tidied up.
pub fn fill(format: &str, v: &Values, multi: u8) -> String {
    let mut out = String::new();
    let mut rest = format;
    // "{Air-Date}" uses a hyphenated date.
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let inner = &rest[start + 1..start + end];
        let (name, spec) = inner.split_once(':').unwrap_or((inner, ""));
        let name_l = name.to_ascii_lowercase();
        let value = if name_l == "air-date" || name_l == "air_date" {
            v.air_date.clone().unwrap_or_default()
        } else {
            token(name, spec, v, multi).unwrap_or_default()
        };
        // Tokens written with dots or underscores put them between words too.
        let value = if name.contains('.') && !name.contains(' ') { value.replace(' ', ".") } else if name.contains('_') { value.replace(' ', "_") } else { value };
        out.push_str(&value);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    tidy(&out)
}

fn tidy(s: &str) -> String {
    let mut t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    for junk in ["()", "[]", "{}"] {
        t = t.replace(junk, "");
    }
    loop {
        let before = t.clone();
        t = t.replace(" -  - ", " - ").replace(" - - ", " - ");
        t = t.trim().trim_end_matches(" -").trim_end_matches('-').trim_start_matches("- ").trim().to_string();
        t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if t == before {
            break;
        }
    }
    t
}

/// Make a name safe for a file or folder: Midgar's colon rule, no characters
/// Windows or DSM refuse, Unicode in one form, no trailing dots or spaces.
pub fn clean_component(s: &str, colon: u8) -> String {
    let s: String = s.nfc().collect();
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            ':' => {
                let next_space = chars.get(i + 1).is_some_and(|n| *n == ' ');
                let prev_space = i > 0 && chars[i - 1] == ' ';
                match colon {
                    0 => {}
                    1 => out.push('-'),
                    2 => out.push_str(" -"),
                    3 => out.push_str(" - "),
                    // Smart: "Dune: Part Two" -> "Dune - Part Two", "10:30" -> "10-30".
                    _ => {
                        if next_space && !prev_space {
                            out.push_str(" -");
                        } else {
                            out.push('-');
                        }
                    }
                }
            }
            '/' | '\\' => out.push('+'),
            '*' | '?' | '"' | '<' | '>' | '|' => {}
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    let t = out.split_whitespace().collect::<Vec<_>>().join(" ");
    t.trim_end_matches(['.', ' ']).trim_start_matches([' ', '.']).to_string()
}

/// Shorten `stem` so `stem + longest suffix` fits in `limit` bytes,
/// cutting the episode title first and never the S/E numbers.
pub fn fit(stem: &str, episode_title: &str, limit: usize) -> String {
    if stem.len() <= limit {
        return stem.to_string();
    }
    let over = stem.len() - limit;
    if !episode_title.is_empty() {
        if let Some(pos) = stem.rfind(episode_title) {
            if episode_title.len() > over + 3 {
                let mut keep = episode_title.len() - over - 3;
                while !episode_title.is_char_boundary(keep) {
                    keep -= 1;
                }
                let short = format!("{}…", episode_title[..keep].trim_end());
                let candidate = format!("{}{}{}", &stem[..pos], short, &stem[pos + episode_title.len()..]);
                if candidate.len() <= limit {
                    return candidate;
                }
            }
            let without = tidy(&format!("{}{}", &stem[..pos], &stem[pos + episode_title.len()..]));
            if without.len() <= limit {
                return without;
            }
        }
    }
    let mut cut = limit;
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    stem[..cut].trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rop(eps: Vec<u32>, titles: &[&str]) -> Values {
        Values {
            series_title: "The Lord of the Rings: The Rings of Power".into(),
            series_year: Some(2022),
            season: Some(3),
            episodes: eps,
            episode_titles: titles.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn midgar_formats() {
        let n = EpisodeNaming::default();
        let name = fill(&n.standard, &rop(vec![4], &["Gundabad"]), n.multi_style);
        assert_eq!(clean_component(&name, n.colon), "The Lord of the Rings - The Rings of Power - S03E04 - Gundabad");
        let name = fill(&n.standard, &rop(vec![1, 2], &["A", "B"]), 5);
        assert_eq!(name, "The Lord of the Rings: The Rings of Power - S03E01-E02 - A + B");
        assert_eq!(fill(&n.season_folder, &rop(vec![1], &[]), 5), "Season 3");
        // Unknown tokens (quality) drop out cleanly.
        let name = fill("{Series Title} - S{season:00}E{episode:00} - {Episode Title} {Quality Full}", &rop(vec![4], &["Gundabad"]), 5);
        assert_eq!(name, "The Lord of the Rings: The Rings of Power - S03E04 - Gundabad");
        // A missing episode title leaves no dangling dash.
        let name = fill(&n.standard, &rop(vec![4], &[]), 5);
        assert_eq!(name, "The Lord of the Rings: The Rings of Power - S03E04");
        let daily = Values { series_title: "The Daily Show".into(), air_date: Some("2026-11-17".into()), episode_titles: vec!["Guest".into()], ..Default::default() };
        assert_eq!(fill(&n.daily, &daily, 5), "The Daily Show - 2026-11-17 - Guest");
    }

    #[test]
    fn movies_and_cleaning() {
        let m = MovieNaming::default();
        let v = Values { movie_title: "Dune: Part Two".into(), movie_year: Some(2024), ..Default::default() };
        assert_eq!(clean_component(&fill(&m.file, &v, 5), m.colon), "Dune - Part Two (2024)");
        assert_eq!(clean_component("What If...?", 4), "What If...".trim_end_matches('.'));
        assert_eq!(clean_component("AC/DC: Live", 4), "AC+DC - Live");
        assert_eq!(clean_component("10:30 Show", 4), "10-30 Show");
    }

    #[test]
    fn long_names_keep_numbers() {
        let title = "A".repeat(300);
        let stem = format!("Show - S01E01 - {title}");
        let short = fit(&stem, &title, 200);
        assert!(short.len() <= 200);
        assert!(short.starts_with("Show - S01E01 - "));
    }
}
