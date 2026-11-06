use std::io::{BufRead, BufReader, Read};
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::nofollow::{open_dir, open_dir_at, open_file_at};
use crate::quality_report::{OpenedReport, QUALITY_DIR};

// Папки и файлы, которые пишет quality_eval из репозитория Nixyon.
const CLIPS_DIR: &str = "clips";
const AUDIO_DIR: &str = "audio";
const SUMMARY: &str = "summary.json";
// Файл клипов набора на сотни тысяч клипов с выравниванием слов доходит до сотен МБ.
const MAX_CLIPS_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_SUMMARY_BYTES: u64 = 8 * 1024 * 1024;
// Звонок на несколько минут в WAV 16 кГц занимает десяток МБ.
const MAX_AUDIO_BYTES: u64 = 64 * 1024 * 1024;
// Строка длиннее не разбирается: такой клип пропускается, страница его не покажет.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Сколько клипов отдаётся за раз.
pub const PAGE: usize = 50;
pub const MAX_QUERY_CHARS: usize = 200;
const MAX_TAGS: usize = 16;
const MAX_EDITS: usize = 20_000;
const MAX_TEXT_CHARS: usize = 20_000;

/// Метки клипов, по которым можно отбирать.
pub const TAGS: [&str; 6] = [
    "errors",
    "empty",
    "failed",
    "cut_start",
    "cut_end",
    "digits",
];

/// Набор, у которого в последнем отчёте есть файл клипов.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClipSet {
    pub dataset: String,
    pub title: String,
    pub has_text: bool,
    pub size: u64,
    pub modified_at: i64,
}

/// Что ищем в клипах набора: метка, текст (в эталоне, распознанном или id) и с какого
/// совпадения отдавать.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipQuery {
    pub tag: Option<String>,
    pub text: Option<String>,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClipPage {
    /// Сколько клипов подошло всего.
    pub total: usize,
    pub offset: usize,
    pub clips: Vec<ClipView>,
}

/// Клип для страницы. Поля из строки файла клипов, лишние отбрасываются.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipView {
    pub id: String,
    pub duration_sec: f64,
    pub reference: String,
    pub hypothesis: String,
    pub word_errors: u64,
    pub words: u64,
    pub speakers: u32,
    #[serde(default)]
    pub expected_speakers: Option<u32>,
    #[serde(default)]
    pub failure: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Правки слов: `["=", слово]`, `["~", эталон, распознано]`, `["-", слово]`, `["+", слово]`.
    #[serde(default)]
    pub edits: Option<Vec<Vec<String>>>,
    /// Номер WAV худшего клипа, если звук сохранён.
    #[serde(default)]
    pub audio: Option<u32>,
}

// Поля summary.json, нужные для списка наборов.
#[derive(Deserialize)]
struct Summary {
    #[serde(default)]
    datasets: Vec<SummaryDataset>,
}

#[derive(Deserialize)]
struct SummaryDataset {
    info: SummaryInfo,
}

#[derive(Deserialize)]
struct SummaryInfo {
    id: String,
    title: String,
    has_text: bool,
}

pub fn is_valid_dataset(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Наборы последнего отчёта, у которых есть файл клипов, в порядке отчёта.
pub fn clip_sets(artifacts_dir: &Path) -> Vec<ClipSet> {
    let Some(report) = report_dir(artifacts_dir) else {
        return Vec::new();
    };
    let Some(summary) = read_summary(&report) else {
        return Vec::new();
    };
    let Ok(clips) = open_dir_at(&report, CLIPS_DIR) else {
        return Vec::new();
    };
    summary
        .datasets
        .into_iter()
        .filter(|d| is_valid_dataset(&d.info.id))
        .filter_map(|d| {
            let (file, size) =
                open_file_at(&clips, &clips_file(&d.info.id), MAX_CLIPS_BYTES).ok()?;
            Some(ClipSet {
                modified_at: file.metadata().ok()?.mtime(),
                dataset: d.info.id,
                title: d.info.title.chars().take(200).collect(),
                has_text: d.info.has_text,
                size,
            })
        })
        .collect()
}

/// Клипы набора, которые подходят под запрос, в порядке файла (худшие раньше).
/// None: набора или файла нет.
pub fn query(artifacts_dir: &Path, dataset: &str, query: &ClipQuery) -> Option<ClipPage> {
    let file = open_clips(artifacts_dir, dataset)?;
    let text = query.text.as_deref().map(folded);
    let mut reader = BufReader::new(file.file);
    let mut line = Vec::new();
    let mut total = 0;
    let mut clips = Vec::new();
    loop {
        line.clear();
        match reader
            .by_ref()
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.len() > MAX_LINE_BYTES {
            skip_rest_of_line(&mut reader);
            continue;
        }
        let Ok(clip) = serde_json::from_slice::<ClipView>(&line) else {
            continue;
        };
        if !matches(&clip, query.tag.as_deref(), text.as_deref()) {
            continue;
        }
        if total >= query.offset && clips.len() < PAGE {
            clips.push(trimmed(clip));
        }
        total += 1;
    }
    Some(ClipPage {
        total,
        offset: query.offset,
        clips,
    })
}

/// Файл клипов набора на скачивание.
pub fn open_clips(artifacts_dir: &Path, dataset: &str) -> Option<OpenedReport> {
    if !is_valid_dataset(dataset) {
        return None;
    }
    let dir = open_dir_at(&report_dir(artifacts_dir)?, CLIPS_DIR).ok()?;
    let name = clips_file(dataset);
    let (file, len) = open_file_at(&dir, &name, MAX_CLIPS_BYTES).ok()?;
    Some(OpenedReport {
        file,
        len,
        content_type: "application/x-ndjson",
        download_name: format!("nixyon-quality-clips-{name}"),
    })
}

/// WAV худшего клипа под номером `n`.
pub fn open_audio(artifacts_dir: &Path, dataset: &str, n: u32) -> Option<OpenedReport> {
    if !is_valid_dataset(dataset) {
        return None;
    }
    let audio = open_dir_at(&report_dir(artifacts_dir)?, AUDIO_DIR).ok()?;
    let dir = open_dir_at(&audio, dataset).ok()?;
    let name = format!("{n}.wav");
    let (file, len) = open_file_at(&dir, &name, MAX_AUDIO_BYTES).ok()?;
    Some(OpenedReport {
        file,
        len,
        content_type: "audio/wav",
        download_name: format!("nixyon-quality-{dataset}-{name}"),
    })
}

fn clips_file(dataset: &str) -> String {
    format!("{dataset}.jsonl")
}

fn report_dir(artifacts_dir: &Path) -> Option<OwnedFd> {
    open_dir_at(&open_dir(artifacts_dir).ok()?, QUALITY_DIR).ok()
}

fn read_summary(report: &OwnedFd) -> Option<Summary> {
    let (file, _) = open_file_at(report, SUMMARY, MAX_SUMMARY_BYTES).ok()?;
    let mut text = String::new();
    file.take(MAX_SUMMARY_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

fn skip_rest_of_line(reader: &mut impl BufRead) {
    loop {
        let Ok(buf) = reader.fill_buf() else { return };
        if buf.is_empty() {
            return;
        }
        if let Some(at) = buf.iter().position(|&b| b == b'\n') {
            reader.consume(at + 1);
            return;
        }
        let len = buf.len();
        reader.consume(len);
    }
}

// Поиск без учёта регистра и разницы между «ё» и «е».
fn folded(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}

fn matches(clip: &ClipView, tag: Option<&str>, text: Option<&str>) -> bool {
    if let Some(tag) = tag
        && !clip.tags.iter().any(|t| t == tag)
    {
        return false;
    }
    match text {
        None => true,
        Some(text) => [&clip.id, &clip.reference, &clip.hypothesis]
            .into_iter()
            .any(|field| folded(field).contains(text)),
    }
}

// Пределы на поля строки: файл пишет код из репозитория, страница не должна тонуть в
// одной огромной строке.
fn trimmed(mut clip: ClipView) -> ClipView {
    let cut = |s: &mut String| {
        if s.chars().count() > MAX_TEXT_CHARS {
            *s = s.chars().take(MAX_TEXT_CHARS).collect();
        }
    };
    cut(&mut clip.reference);
    cut(&mut clip.hypothesis);
    if let Some(failure) = clip.failure.as_mut() {
        cut(failure);
    }
    clip.tags.truncate(MAX_TAGS);
    if let Some(edits) = clip.edits.as_mut() {
        edits.truncate(MAX_EDITS);
    }
    clip
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "nixyon-quality-clips-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join(QUALITY_DIR).join(CLIPS_DIR)).unwrap();
            Self(path)
        }

        fn report(&self) -> std::path::PathBuf {
            self.0.join(QUALITY_DIR)
        }

        fn clips(&self, dataset: &str, lines: &[String]) {
            std::fs::write(
                self.report()
                    .join(CLIPS_DIR)
                    .join(format!("{dataset}.jsonl")),
                lines.join("\n") + "\n",
            )
            .unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn line(id: &str, reference: &str, heard: &str, tags: &[&str]) -> String {
        serde_json::json!({
            "dataset": "cv", "id": id, "duration_sec": 2.0, "processing_sec": 0.1,
            "reference": reference, "hypothesis": heard, "speakers": 1,
            "word_errors": 1, "words": 2, "failure": null, "tags": tags,
            "edits": [["=", "раз"], ["~", "два", "три"]], "audio": 0
        })
        .to_string()
    }

    fn ask(tag: Option<&str>, text: Option<&str>, offset: usize) -> ClipQuery {
        ClipQuery {
            tag: tag.map(Into::into),
            text: text.map(Into::into),
            offset,
        }
    }

    #[test]
    fn clips_are_filtered_by_tag_and_text_in_file_order() {
        let dir = Dir::new("filter");
        dir.clips(
            "cv",
            &[
                line("a", "Ёлка стоит", "елка", &["errors", "cut_end"]),
                line("b", "раз два", "раз три", &["errors"]),
                "не json".into(),
                line("c", "КОТ", "кот", &[]),
            ],
        );
        let all = query(&dir.0, "cv", &ClipQuery::default()).unwrap();
        assert_eq!(all.total, 3, "битая строка пропускается");
        assert_eq!(all.clips[0].id, "a");
        assert_eq!(all.clips[0].edits.as_ref().unwrap()[1], ["~", "два", "три"]);
        let cut = query(&dir.0, "cv", &ask(Some("cut_end"), None, 0)).unwrap();
        assert_eq!(cut.total, 1);
        let found = query(&dir.0, "cv", &ask(None, Some("елк"), 0)).unwrap();
        assert_eq!(found.clips[0].id, "a", "ё и регистр не мешают");
        let by_id = query(&dir.0, "cv", &ask(None, Some("c"), 0)).unwrap();
        assert_eq!(by_id.total, 1);
        let later = query(&dir.0, "cv", &ask(None, None, 2)).unwrap();
        assert_eq!((later.total, later.clips.len()), (3, 1));
        assert_eq!(later.clips[0].id, "c");
        assert!(query(&dir.0, "nope", &ClipQuery::default()).is_none());
        assert!(query(&dir.0, "../cv", &ClipQuery::default()).is_none());
    }

    #[test]
    fn a_page_holds_at_most_fifty_clips_and_counts_the_rest() {
        let dir = Dir::new("page");
        let lines: Vec<String> = (0..PAGE + 7)
            .map(|n| line(&format!("k{n}"), "раз", "два", &["errors"]))
            .collect();
        dir.clips("cv", &lines);
        let page = query(&dir.0, "cv", &ClipQuery::default()).unwrap();
        assert_eq!((page.total, page.clips.len()), (PAGE + 7, PAGE));
    }

    #[test]
    fn an_overlong_line_is_skipped_and_the_next_one_still_read() {
        let dir = Dir::new("long");
        let huge = line("big", &"а".repeat(MAX_LINE_BYTES), "", &[]);
        dir.clips("cv", &[huge, line("next", "раз", "раз", &[])]);
        let page = query(&dir.0, "cv", &ClipQuery::default()).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.clips[0].id, "next");
    }

    #[test]
    fn sets_come_from_the_summary_where_a_clip_file_exists() {
        let dir = Dir::new("sets");
        assert!(clip_sets(&dir.0).is_empty());
        std::fs::write(
            dir.report().join(SUMMARY),
            r#"{"datasets":[
                {"info":{"id":"cv","title":"Common Voice","has_text":true}},
                {"info":{"id":"gone","title":"Нет файла","has_text":true}},
                {"info":{"id":"../x","title":"Плохое имя","has_text":true}}
            ]}"#,
        )
        .unwrap();
        dir.clips("cv", &[line("a", "раз", "раз", &[])]);
        let sets = clip_sets(&dir.0);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].dataset, "cv");
        assert_eq!(sets[0].title, "Common Voice");
        let opened = open_clips(&dir.0, "cv").unwrap();
        assert_eq!(opened.download_name, "nixyon-quality-clips-cv.jsonl");
    }

    #[test]
    fn audio_is_served_by_number_and_links_are_refused() {
        let dir = Dir::new("audio");
        let audio = dir.report().join(AUDIO_DIR).join("cv");
        std::fs::create_dir_all(&audio).unwrap();
        std::fs::write(audio.join("0.wav"), b"RIFF").unwrap();
        let opened = open_audio(&dir.0, "cv", 0).unwrap();
        assert_eq!((opened.len, opened.content_type), (4, "audio/wav"));
        assert!(open_audio(&dir.0, "cv", 1).is_none());
        assert!(open_audio(&dir.0, "../cv", 0).is_none());
        std::os::unix::fs::symlink(audio.join("0.wav"), audio.join("2.wav")).unwrap();
        assert!(open_audio(&dir.0, "cv", 2).is_none());
    }
}
