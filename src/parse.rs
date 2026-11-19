//! Reads a release file name: title, year, season/episode, air date or
//! anime episode number. Junk like "www.site.org - " and "[TGx]" is dropped.

use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    /// Season + episode numbers (S03E01, 3x01).
    Episode,
    /// Shows named by air date.
    Daily,
    /// Anime numbered straight through (" - 25").
    Absolute,
    /// A whole season with no episode number (folder names).
    SeasonPack,
    Movie,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Parsed {
    pub title: String,
    pub year: Option<u16>,
    pub kind: Kind,
    pub season: Option<u32>,
    pub episodes: Vec<u32>,
    pub absolute: Vec<u32>,
    /// YYYY-MM-DD for daily shows.
    pub air_date: Option<String>,
    /// Started with a [Group] tag, as anime releases do.
    pub anime: bool,
    pub sample: bool,
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

macro_rules! rx {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static C: OnceLock<Regex> = OnceLock::new();
            re(&C, $pat)
        }
    };
}

rx!(site_prefix, r"(?i)^\s*(?:www\.)?[a-z0-9][a-z0-9-]*\.(?:org|com|net|to|me|info|cc|tv|xyz|io|ru|se|li|ws|lol|nu|in|pw|unblocked\.[a-z]+)\s*[-–_]+\s*");
rx!(lead_group, r"^\s*\[([^\]]*)\]\s*");
rx!(brackets, r"\[[^\]]*\]");
rx!(sxxexx, r"(?i)\bS(\d{1,2})\s?E(\d{1,3})((?:\s?-?\s?E\d{1,3}|-\d{1,3}(?:\b|$))*)");
rx!(more_eps, r"(?i)E?(\d{1,3})");
rx!(nxnn, r"(?i)\b(\d{1,2})x(\d{2,3})\b");
rx!(daily, r"\b((?:19|20)\d{2})[ .-](\d{2})[ .-](\d{2})\b");
rx!(season_only, r"(?i)\b(?:S(\d{1,2})|Season\s?(\d{1,2}))\b");
rx!(anime_ep, r"(?i)(?:\s-\s|\s#|\sE|\sEP\.?\s?)(\d{1,4})(?:v\d)?(?:\s|$|\(|\[)");
rx!(anime_bare, r"\s(\d{1,4})(?:v\d)?\s*(?:\(|\[|$)");
rx!(year, r"\b(19[2-9]\d|20[0-4]\d)\b");
rx!(paren_year, r"\(\s*(19[2-9]\d|20[0-4]\d)\s*\)");
rx!(quality, r"(?i)\b(?:2160p|1080[pi]|720p|576p|480p|4k|uhd|web[ -]?dl|webrip|web|blu-?ray|bdrip|brrip|bdremux|remux|hdtv|pdtv|dvdrip|dvd|hdrip|x26[45]|h\s?26[45]|hevc|av1|xvid|divx|10bit|8bit|hdr10?|dv|dolby|atmos|ddp?\s?5\s?1|aac\s?2\s?0|proper|repack|internal|extended|unrated|directors\s?cut|amzn|nf|dsnp|hmax|atvp|pcok|hulu|multi|subbed|dubbed|complete)\b");
rx!(sample_word, r"(?i)\bsample\b");

/// Turn dots/underscores into spaces, but keep things like "5.1" and "Mr." readable.
fn spaced(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            out.push(' ');
        } else if c == '.' {
            // "5.1" and "2.0" stay; "2012.2009" (two years) splits.
            let run_before = chars[..i].iter().rev().take_while(|c| c.is_ascii_digit()).count();
            let run_after = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
            if run_before > 0 && run_after > 0 && run_before <= 2 && run_after <= 2 {
                out.push('.');
            } else {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    collapse(&out)
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip separators left at the end of a title ("Show -", "Show (").
fn tidy_title(s: &str) -> String {
    let t = s.trim().trim_end_matches(|c: char| c == '-' || c == '(' || c == '[' || c == '–' || c == ',' || c.is_whitespace());
    let t = t.trim_start_matches(|c: char| c == '-' || c.is_whitespace());
    collapse(t)
}

/// Parse a file or folder name (no extension).
pub fn parse_name(name: &str) -> Parsed {
    let mut p = Parsed { sample: sample_word().is_match(name), ..Default::default() };
    let mut s = name.to_string();
    // Leading junk: site names and [Group] tags, possibly repeated.
    loop {
        if let Some(m) = site_prefix().find(&s) {
            s = s[m.end()..].to_string();
            continue;
        }
        if let Some(c) = lead_group().captures(&s) {
            let inner = c.get(1).map(|m| m.as_str()).unwrap_or("");
            // "[2024]" isn't a group tag.
            if !inner.chars().all(|ch| ch.is_ascii_digit()) {
                p.anime = true;
            }
            s = s[c.get(0).unwrap().end()..].to_string();
            continue;
        }
        break;
    }
    let s = brackets().replace_all(&s, " ");
    let s = spaced(&s);

    // Find the first marker; the title is what comes before it.
    let mut cut: Option<usize> = None;
    if let Some(c) = sxxexx().captures(&s) {
        p.kind = Kind::Episode;
        p.season = c[1].parse().ok();
        let mut eps: Vec<u32> = vec![c[2].parse().unwrap_or(0)];
        let rest = c.get(3).map(|m| m.as_str()).unwrap_or("");
        for e in more_eps().captures_iter(rest) {
            if let Ok(n) = e[1].parse::<u32>() {
                eps.push(n);
            }
        }
        // "E01-03" means 1, 2, 3.
        if eps.len() == 2 && eps[1] > eps[0] + 1 && rest.contains('-') && !rest.to_ascii_uppercase().contains('E') {
            eps = (eps[0]..=eps[1]).collect();
        }
        eps.sort_unstable();
        eps.dedup();
        p.episodes = eps;
        cut = Some(c.get(0).unwrap().start());
    } else if let Some(c) = nxnn().captures(&s) {
        p.kind = Kind::Episode;
        p.season = c[1].parse().ok();
        p.episodes = vec![c[2].parse().unwrap_or(0)];
        cut = Some(c.get(0).unwrap().start());
    } else if let Some(c) = daily().captures(&s) {
        let (y, m, d): (u16, u8, u8) = (c[1].parse().unwrap_or(0), c[2].parse().unwrap_or(0), c[3].parse().unwrap_or(0));
        if (1..=12).contains(&m) && (1..=31).contains(&d) {
            p.kind = Kind::Daily;
            p.air_date = Some(format!("{y:04}-{m:02}-{d:02}"));
            cut = Some(c.get(0).unwrap().start());
        }
    }
    if cut.is_none() && p.anime {
        let c = anime_ep().captures(&s).or_else(|| anime_bare().captures(&s));
        if let Some(c) = c {
            let n: u32 = c[1].parse().unwrap_or(0);
            // A 4-digit "episode" that looks like a year isn't one.
            if n > 0 && !(1920..=2049).contains(&n) {
                p.kind = Kind::Absolute;
                p.absolute = vec![n];
                cut = Some(c.get(0).unwrap().start());
            }
        }
    }
    if cut.is_none() {
        if let Some(c) = season_only().captures(&s) {
            p.kind = Kind::SeasonPack;
            p.season = c.get(1).or(c.get(2)).and_then(|m| m.as_str().parse().ok());
            cut = Some(c.get(0).unwrap().start());
        }
    }

    let head_end = cut.unwrap_or(s.len());
    // Quality words also end the title.
    let q_at = quality().find(&s[..head_end]).map(|m| m.start());
    let mut title_part = s[..q_at.unwrap_or(head_end).min(head_end)].to_string();

    // A year in parentheses or at the end of the title belongs to it.
    if let Some(c) = paren_year().captures(&title_part) {
        p.year = c[1].parse().ok();
        let m = c.get(0).unwrap();
        title_part = format!("{} {}", &title_part[..m.start()], &title_part[m.end()..]);
    }
    let had_paren_year = p.year.is_some();
    if cut.is_none() {
        // Movies: the title ends at the last plausible year (not the first word).
        let years: Vec<regex::Match> = year().find_iter(&title_part).filter(|m| m.start() > 0).collect();
        if let Some(m) = years.last().filter(|_| !had_paren_year) {
            p.year = m.as_str().parse().ok();
            title_part.truncate(m.start());
        }
        p.kind = if p.year.is_some() || q_at.is_some() { Kind::Movie } else { Kind::Unknown };
    } else if p.year.is_none() {
        // "Shogun 2024 S01E01": a trailing year names the show's version.
        let t = title_part.trim_end();
        if let Some(m) = year().find_iter(t).filter(|m| m.start() > 0).last() {
            if m.end() == t.len() {
                p.year = m.as_str().parse().ok();
                title_part = t[..m.start()].to_string();
            }
        }
    }
    p.title = tidy_title(&title_part);
    p
}

/// Parse a path: the file name first, then its folder for what the file name lacks.
pub fn parse_path(path: &Path) -> Parsed {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let mut p = parse_name(stem);
    let parent = path.parent().and_then(|d| d.file_name()).and_then(|n| n.to_str()).unwrap_or("");
    if parent.is_empty() {
        return p;
    }
    let f = parse_name(parent);
    if p.title.is_empty() || (p.kind == Kind::Unknown && f.kind != Kind::Unknown) {
        // "Show.S03.1080p/05 - Title.mkv" or "Movie.2024.1080p/abc123.mkv".
        if p.title.is_empty() || p.kind == Kind::Unknown {
            p.title = f.title.clone();
            p.year = p.year.or(f.year);
        }
        if f.kind == Kind::SeasonPack || f.kind == Kind::Episode {
            if let Some(n) = leading_number(stem) {
                p.kind = Kind::Episode;
                p.season = f.season;
                p.episodes = vec![n];
            }
        } else if p.kind == Kind::Unknown {
            p.kind = f.kind.clone();
        }
        p.anime |= f.anime;
    }
    if p.kind == Kind::Episode && p.year.is_none() {
        p.year = f.year.filter(|_| normalize(&f.title) == normalize(&p.title));
    }
    p.sample |= sample_word().is_match(parent);
    p
}

fn leading_number(stem: &str) -> Option<u32> {
    static C: OnceLock<Regex> = OnceLock::new();
    let r = re(&C, r"(?i)^\s*(?:E|EP|Episode\s?)?(\d{1,3})\b");
    r.captures(stem).and_then(|c| c[1].parse().ok())
}

/// Compare titles loosely: case, accents, punctuation and "&" don't matter,
/// and "S.W.A.T." equals "SWAT".
pub fn normalize(title: &str) -> String {
    let lower: String = title
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
        .replace('&', " and ")
        .replace(['\'', '’', '`'], "");
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in lower.chars() {
        if c.is_alphanumeric() {
            cur.push(c);
        } else if !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    // Join runs of single letters: "s w a t" -> "swat".
    let mut out: Vec<(String, bool)> = Vec::new();
    for w in words {
        let single = w.chars().count() == 1 && w.chars().all(|c| c.is_alphabetic());
        match out.last_mut() {
            Some((last, true)) if single => last.push_str(&w),
            _ => out.push((w, single)),
        }
    }
    out.into_iter().map(|(w, _)| w).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_junk_and_season_episode() {
        let p = parse_name("www.UIndex.org    -    The.Lord.of.the.Rings.The.Rings.of.Power.S03E01.1080p.HEVC.x265-MeGusta");
        assert_eq!(p.title, "The Lord of the Rings The Rings of Power");
        assert_eq!(p.kind, Kind::Episode);
        assert_eq!((p.season, p.episodes.clone()), (Some(3), vec![1]));
        let p = parse_name("The.Lord.of.the.Rings.The.Rings.of.Power.S02E05.Halls.of.Stone.1080p.HEVC.x265-MeGusta[TGx]");
        assert_eq!(p.title, "The Lord of the Rings The Rings of Power");
        assert_eq!((p.season, p.episodes), (Some(2), vec![5]));
        assert!(!p.anime);
    }

    #[test]
    fn n_x_nn_and_multi() {
        let p = parse_name("The Vampire Diaries 08x16  I Was Feeling Epic");
        assert_eq!(p.title, "The Vampire Diaries");
        assert_eq!((p.season, p.episodes), (Some(8), vec![16]));
        let p = parse_name("Show.Name.S01E01E02.720p");
        assert_eq!(p.episodes, vec![1, 2]);
        let p = parse_name("Show.Name.S01E01-E03.720p");
        assert_eq!(p.episodes, vec![1, 3]);
        let p = parse_name("Show Name - S01E05-06 - Title");
        assert_eq!(p.episodes, vec![5, 6]);
    }

    #[test]
    fn years_and_movies() {
        let p = parse_name("Dune.Part.Two.2024.1080p.WEB-DL.x264");
        assert_eq!((p.title.as_str(), p.year, p.kind), ("Dune Part Two", Some(2024), Kind::Movie));
        let p = parse_name("Blade Runner 2049 (2017) 2160p");
        assert_eq!((p.title.as_str(), p.year), ("Blade Runner 2049", Some(2017)));
        let p = parse_name("2012.2009.1080p.BluRay");
        assert_eq!((p.title.as_str(), p.year), ("2012", Some(2009)));
        let p = parse_name("Shogun.2024.S01E01.Anjin.1080p");
        assert_eq!((p.title.as_str(), p.year, p.season), ("Shogun", Some(2024), Some(1)));
        let p = parse_name("Scrubs (2001) - S01E01 - My First Day");
        assert_eq!((p.title.as_str(), p.year), ("Scrubs", Some(2001)));
    }

    #[test]
    fn daily_and_anime() {
        let p = parse_name("The.Daily.Show.2026.11.17.Guest.Name.1080p");
        assert_eq!((p.title.as_str(), p.kind, p.air_date.as_deref()), ("The Daily Show", Kind::Daily, Some("2026-11-17")));
        let p = parse_name("[SubsPlease] Sousou no Frieren - 25 (1080p) [ABCD1234]");
        assert_eq!((p.title.as_str(), p.kind, p.absolute.clone()), ("Sousou no Frieren", Kind::Absolute, vec![25]));
        assert!(p.anime);
        let p = parse_name("[Group] One Piece 1085 [1080p]");
        assert_eq!((p.title.as_str(), p.absolute), ("One Piece", vec![1085]));
    }

    #[test]
    fn folders_fill_gaps() {
        let p = parse_path(Path::new("/x/Show.Name.S03.1080p.WEB/05 - Some Title.mkv"));
        assert_eq!((p.title.as_str(), p.season, p.episodes), ("Show Name", Some(3), vec![5]));
        let p = parse_path(Path::new("/x/Movie.Name.2019.1080p/abc123.mkv"));
        assert_eq!((p.title.as_str(), p.year), ("Movie Name", Some(2019)));
        let p = parse_path(Path::new("/x/Show.S01E01.720p/Sample/show.s01e01.sample.mkv"));
        assert!(p.sample);
    }

    #[test]
    fn normalizing() {
        assert_eq!(normalize("The Lord of the Rings: The Rings of Power"), normalize("The Lord of the Rings The Rings of Power"));
        assert_eq!(normalize("S.W.A.T."), "swat");
        assert_eq!(normalize("Law & Order"), "law and order");
        assert_eq!(normalize("Shōgun"), "shogun");
        assert_eq!(normalize("It's Always Sunny"), "its always sunny");
        assert_eq!(normalize("Marvel's Daredevil"), normalize("Marvels Daredevil"));
    }
}
