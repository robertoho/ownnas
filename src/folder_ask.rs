//! Ask: a question in plain language ("where are the car wash permits from 2021?") answered with
//! the few described folders (`folder_notes.rs`) that match it best, each with the files inside
//! that fit the question most. No chatbot, and nothing leaves the server.
//!
//! Folders are ranked by meaning, with a small sentence model run on the processor (`ml::text`):
//! each description is turned into numbers once, in the background at the indexing speed, and a
//! question is compared with them all. Words of the question found in a description, in the
//! names of the files a folder holds, or in their text (the full-text index) count too, and they
//! alone pick the files shown inside each folder. With the model off or not downloaded yet, Ask
//! ranks by those words only.
//!
//! Which model depends on the languages an administrator turns on (`model_for`): a small English
//! one when English is the only one, otherwise one multilingual model for all of them, so a
//! question in Spanish finds a description written in Polish.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::access::{Access, Need};
use crate::agent_keys::within;
use crate::folder_notes::{self, fold, Map, Note};
use crate::media_index::Speed;

/// "1" when Ask's model is switched on (it is downloaded then).
pub const SETTING_ON: &str = "ask_on";
/// The languages turned on, as codes separated by commas ("en,pl,es").
pub const SETTING_LANGUAGES: &str = "ask_languages";

/// The languages Settings offers. Every one is covered by the multilingual model.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("pl", "Polish"),
    ("es", "Spanish"),
    ("de", "German"),
    ("fr", "French"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("ca", "Catalan"),
    ("cs", "Czech"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("hr", "Croatian"),
    ("sr", "Serbian"),
    ("bg", "Bulgarian"),
    ("ro", "Romanian"),
    ("hu", "Hungarian"),
    ("el", "Greek"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("et", "Estonian"),
    ("fi", "Finnish"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("no", "Norwegian"),
    ("uk", "Ukrainian"),
    ("ru", "Russian"),
    ("tr", "Turkish"),
    ("ar", "Arabic"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("zh", "Chinese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("vi", "Vietnamese"),
    ("th", "Thai"),
    ("id", "Indonesian"),
];

/// One of Ask's sentence models, as Settings describes it.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextModel {
    pub id: &'static str,
    pub name: &'static str,
    pub multilingual: bool,
    /// The languages it understands, in words.
    pub languages: &'static str,
    /// Bytes to download (the model and its tokenizer).
    pub size: u64,
    /// Why this one, for Settings.
    pub why: &'static str,
}

pub static ENGLISH: TextModel = TextModel {
    id: "all-minilm-l6-v2",
    name: "all-MiniLM-L6-v2",
    multilingual: false,
    languages: "English",
    size: 22_972_370 + 231_508,
    why: "Small and quick (23 MB, a few milliseconds a sentence): enough when descriptions and questions are all in English.",
};

pub static MULTILINGUAL: TextModel = TextModel {
    id: "multilingual-e5-small",
    name: "multilingual-e5-small",
    multilingual: true,
    languages: "about 100 languages, including English, Polish, Spanish, German, French, Italian, Portuguese, Dutch, Czech, Ukrainian, Russian, Turkish, Arabic, Hindi, Chinese, Japanese and Korean",
    size: 118_308_185 + 17_082_730,
    why: "One model for every language turned on, made for finding texts that answer a question, so a question in one language finds a description written in another. It covers twice the languages of paraphrase-multilingual-MiniLM-L12-v2 at the same size, and finds better.",
};

/// The model for the languages turned on: English alone needs only the small English model;
/// any other language needs the multilingual one (for all of them, English included).
pub fn model_for(languages: &[String]) -> &'static TextModel {
    if languages.iter().all(|code| code == "en") {
        &ENGLISH
    } else {
        &MULTILINGUAL
    }
}

/// The languages saved in Settings (English when none are), known codes only.
pub fn languages(conn: &Connection) -> Vec<String> {
    let saved = crate::db::get_setting(conn, SETTING_LANGUAGES).ok().flatten().unwrap_or_default();
    parse_languages(&saved)
}

pub fn parse_languages(text: &str) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    for code in text.split(',').map(|code| code.trim().to_ascii_lowercase()) {
        if LANGUAGES.iter().any(|(known, _)| *known == code) && !codes.contains(&code) {
            codes.push(code);
        }
    }
    if codes.is_empty() {
        codes.push("en".into());
    }
    codes
}

pub fn enabled(conn: &Connection) -> bool {
    crate::db::get_setting(conn, SETTING_ON).ok().flatten().as_deref() == Some("1")
}

/// The vectors of descriptions, by model and by the text they were made from, in the index
/// database (they can be made again). Keyed by text, a renamed or moved folder needs nothing new
/// unless its name changed.
pub fn ensure(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS idx.text_vectors (
            model TEXT NOT NULL,
            hash TEXT NOT NULL,
            vector BLOB NOT NULL,
            PRIMARY KEY (model, hash)
        );",
    )
    .map_err(|_| "Could not create the description vectors".to_string())
}

/// What the model reads for a folder: its name (and the one above, for context) and its
/// description.
pub fn passage(path: &str, description: &str) -> String {
    let mut names: Vec<&str> = path.rsplit('/').take(2).collect();
    names.reverse();
    format!("{}: {}", names.join(" / "), description.trim())
}

fn hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn to_blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn from_blob(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4).map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])).collect()
}

pub fn store_vector(conn: &Connection, model: &str, text: &str, vector: &[f32]) -> Result<(), String> {
    conn.execute(
        "INSERT INTO text_vectors(model, hash, vector) VALUES (?1, ?2, ?3) ON CONFLICT(model, hash) DO UPDATE SET vector = excluded.vector",
        params![model, hash(text), to_blob(vector)],
    )
    .map(|_| ())
    .map_err(|_| "Could not save a description's vector".to_string())
}

fn vectors(conn: &Connection, model: &str) -> Result<HashMap<String, Vec<f32>>, String> {
    let mut stmt = conn.prepare("SELECT hash, vector FROM text_vectors WHERE model = ?1").map_err(|err| err.to_string())?;
    let rows = stmt.query_map([model], |row| Ok((row.get::<_, String>(0)?, from_blob(&row.get::<_, Vec<u8>>(1)?)))).map_err(|err| err.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// The notes Ask can answer with: described, outside the Trash, not left out.
fn askable(notes: &[Note], map: &Map) -> Vec<Note> {
    notes.iter().filter(|note| !note.description.is_empty() && !map.left_out(&note.path)).cloned().collect()
}

/// A word of the question, as the indexes hold words: lower case (the text of files) and
/// without accents (descriptions). Long words are cut to their start, so "pozwolenia" also
/// finds "pozwolenie" and "pozwoleń" ("pozwole").
#[derive(Clone, Debug, PartialEq)]
pub struct Keyword {
    pub lower: String,
    pub folded: String,
}

/// Words that say nothing about what is looked for, in the languages most likely to be asked
/// in. Other languages simply keep theirs (they weigh little next to the meaning).
const STOPWORDS: &[&str] = &[
    // English
    "the", "and", "for", "with", "from", "that", "this", "these", "those", "are", "was", "were", "where", "what", "which", "who", "whom", "when", "how", "why", "can", "could", "find", "show", "give", "list", "any", "all", "our", "your", "have", "has", "had", "about", "into", "there", "here", "some", "kept", "keep", "stored", "files", "file", "folder", "folders", "documents", "document", "docs", "please", "need", "want", "look", "looking", "did", "does", "will", "would", "should", "they", "them", "their", "its", "not", "but", "you",
    // Polish
    "gdzie", "jest", "są", "sa", "jak", "jaki", "jakie", "jaka", "które", "ktore", "który", "ktory", "która", "ktora", "czy", "dla", "oraz", "albo", "lub", "się", "sie", "nie", "tak", "ale", "pod", "nad", "przy", "przez", "znajdę", "znajde", "znaleźć", "znalezc", "pliki", "plik", "folder", "foldery", "katalog", "dokumenty", "dokument", "mamy", "mam", "był", "byl", "była", "byla", "były", "byly", "tym", "tego", "ten", "ta", "to", "te", "co", "od", "do", "na", "ze", "po", "za", "we", "mój", "moje", "nasze", "nasz",
    // Spanish
    "dónde", "donde", "está", "esta", "están", "estan", "qué", "que", "cuál", "cual", "cuáles", "cuales", "cómo", "como", "los", "las", "del", "con", "por", "para", "una", "uno", "unos", "unas", "sus", "nuestros", "nuestras", "mis", "son", "hay", "archivos", "archivo", "carpeta", "carpetas", "documentos", "documento", "encontrar", "busco", "tengo", "tenemos", "sobre", "entre", "desde", "hasta",
    // German, French, Italian, Portuguese
    "wo", "sind", "ist", "die", "der", "das", "und", "mit", "für", "fur", "von", "den", "dem", "ein", "eine", "où", "ou", "sont", "les", "des", "une", "pour", "avec", "dans", "est", "dove", "sono", "gli", "della", "delle", "onde", "estão", "estao", "dos", "das", "uma", "com", "pelo", "pela",
];

pub fn keywords(question: &str) -> Vec<Keyword> {
    let mut out: Vec<Keyword> = Vec::new();
    let lower = question.to_lowercase();
    for word in lower.split(|c: char| !c.is_alphanumeric()) {
        let length = word.chars().count();
        if length < 3 && !(length >= 2 && word.chars().all(|c| c.is_ascii_digit())) {
            continue;
        }
        if STOPWORDS.contains(&word) || STOPWORDS.contains(&fold(word).as_str()) {
            continue;
        }
        // Years and numbers are kept whole; words are cut to about two thirds, four letters at least.
        let keep = if word.chars().all(|c| c.is_ascii_digit()) || length <= 4 { length } else { (length * 2).div_ceil(3).max(4) };
        let stem: String = word.chars().take(keep).collect();
        let keyword = Keyword { folded: fold(&stem), lower: stem };
        if !out.contains(&keyword) {
            out.push(keyword);
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundFile {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub size: u64,
    pub modified: i64,
    /// The words around what matched in its text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundFolder {
    pub path: String,
    pub name: String,
    pub description: String,
    pub score: f32,
    pub files: Vec<FoundFile>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    /// Ranked by meaning (the model was used), or by words alone.
    pub by_meaning: bool,
    pub folders: Vec<FoundFolder>,
}

/// The question as the model understood it: which model, and its numbers.
pub struct Meaning<'a> {
    pub model: &'a str,
    pub vector: &'a [f32],
}

/// Folders answered at most, and files shown in each.
const MOST_FOLDERS: usize = 5;
const FEWEST_FOLDERS: usize = 3;
const FILES_EACH: usize = 3;
/// Files found per word of the question, in names and in text.
const PER_WORD: i64 = 5000;

/// Answers `question` for an account, among the folders under `scope` (or the one it is in).
#[cfg(test)]
pub fn ask(conn: &Connection, root: &Path, access: &Access, question: &str, scope: &str, meaning: Option<Meaning>) -> Result<Answer, String> {
    let reachable = |path: &str| access.holds(path) && (!access.limited() || access.check(root, path, Need::Read).is_ok());
    ask_where(conn, root, question, scope, meaning, &reachable)
}

/// The same, for whoever `reachable` says may see a folder or file (an account, an agent key).
pub fn ask_where(conn: &Connection, root: &Path, question: &str, scope: &str, meaning: Option<Meaning>, reachable: &dyn Fn(&str) -> bool) -> Result<Answer, String> {
    let notes = folder_notes::all(conn)?;
    let map = Map::new(&notes);
    let candidates: Vec<Note> = askable(&notes, &map)
        .into_iter()
        .filter(|note| within(scope, &note.path) || within(&note.path, scope))
        .filter(|note| reachable(&note.path))
        .collect();
    if candidates.is_empty() {
        return Ok(Answer { by_meaning: meaning.is_some(), folders: Vec::new() });
    }
    let words = keywords(question);

    // Files whose names or text hold the question's words, each counted for the described
    // folder it belongs to.
    let mut file_hits: HashMap<String, (f32, Option<String>)> = HashMap::new();
    let owned: HashSet<&str> = candidates.iter().map(|note| note.path.as_str()).collect();
    for word in &words {
        let mut in_text = conn
            .prepare("SELECT c.path FROM content_fts x JOIN content_text c ON c.rowid = x.rowid WHERE x.lower GLOB ?1 LIMIT ?2")
            .ok();
        let text_paths: Vec<String> = in_text
            .as_mut()
            .and_then(|stmt| stmt.query_map(params![glob(&word.lower), PER_WORD], |row| row.get::<_, String>(0)).ok().map(|rows| rows.filter_map(Result::ok).collect()))
            .unwrap_or_default();
        let mut by_name = conn.prepare("SELECT path FROM file_index WHERE dir = 0 AND instr(name_lower, ?1) > 0 LIMIT ?2").map_err(|err| err.to_string())?;
        let name_paths: Vec<String> = by_name.query_map(params![word.lower, PER_WORD], |row| row.get::<_, String>(0)).map_err(|err| err.to_string())?.filter_map(Result::ok).collect();
        for (paths, weight, from_text) in [(text_paths, 1.0f32, true), (name_paths, 1.5f32, false)] {
            for path in paths {
                if map.owner(&path).is_some_and(|owner| owned.contains(owner)) {
                    let hit = file_hits.entry(path).or_insert((0.0, None));
                    hit.0 += weight;
                    if from_text && hit.1.is_none() {
                        hit.1 = Some(word.lower.clone());
                    }
                }
            }
        }
    }
    let mut evidence: HashMap<&str, f32> = HashMap::new();
    let mut files_of: HashMap<&str, Vec<(&str, f32)>> = HashMap::new();
    let per_file = words.len().max(1) as f32 * 1.5;
    for (path, (score, _)) in &file_hits {
        if let Some(owner) = map.owner(path) {
            *evidence.entry(owner).or_default() += score / per_file;
            files_of.entry(owner).or_default().push((path.as_str(), *score));
        }
    }

    let vectors = match &meaning {
        Some(meaning) => vectors(conn, meaning.model)?,
        None => HashMap::new(),
    };
    let mut ranked: Vec<(f32, &Note)> = candidates
        .iter()
        .filter_map(|note| {
            let text = passage(&note.path, &note.description);
            let folded = fold(&format!("{} {}", note.path, note.description));
            let lexical = if words.is_empty() { 0.0 } else { words.iter().filter(|word| folded.contains(&word.folded)).count() as f32 / words.len() as f32 };
            let found = evidence.get(note.path.as_str()).copied().unwrap_or(0.0).min(3.0) / 3.0;
            match &meaning {
                Some(meaning) => {
                    // One written a moment ago may not be read yet: its words still count.
                    let close = vectors.get(&hash(&text)).map_or(0.0, |vector| crate::smart::similarity(meaning.vector, vector));
                    Some((close + 0.12 * lexical + 0.08 * found, note))
                }
                None => Some((lexical + 0.5 * found, note)).filter(|(score, _)| *score > 0.0),
            }
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));

    let mut folders: Vec<FoundFolder> = Vec::new();
    for (score, note) in ranked {
        if folders.len() >= MOST_FOLDERS {
            break;
        }
        // Past the first few, only folders nearly as close as those.
        if folders.len() >= FEWEST_FOLDERS && score < folders[FEWEST_FOLDERS - 1].score - 0.03 {
            break;
        }
        if !root.join(&note.path).is_dir() {
            continue;
        }
        let mut picks: Vec<(&str, f32)> = files_of.get(note.path.as_str()).cloned().unwrap_or_default();
        picks.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let mut files: Vec<FoundFile> = Vec::new();
        for (path, _) in picks {
            if files.len() >= FILES_EACH {
                break;
            }
            if reachable(path) {
                if let Some(mut file) = file_row(conn, path)? {
                    if let Some(Some(word)) = file_hits.get(path).map(|hit| &hit.1) {
                        file.snippet = snippet(conn, path, word);
                    }
                    files.push(file);
                }
            }
        }
        if files.len() < FILES_EACH {
            // The newest of its own files, for a folder the words did not reach into.
            for file in newest(conn, &note.path, &map, 200)? {
                if files.len() >= FILES_EACH {
                    break;
                }
                if !files.iter().any(|known| known.path == file.path) && reachable(&file.path) {
                    files.push(file);
                }
            }
        }
        folders.push(FoundFolder {
            name: note.path.rsplit('/').next().unwrap_or(&note.path).to_string(),
            path: note.path.clone(),
            description: note.description.clone(),
            score,
            files,
        });
    }
    Ok(Answer { by_meaning: meaning.is_some(), folders })
}

fn glob(text: &str) -> String {
    let mut out = String::from("*");
    for c in text.chars() {
        if matches!(c, '*' | '?' | '[') {
            out.push('[');
            out.push(c);
            out.push(']');
        } else {
            out.push(c);
        }
    }
    out.push('*');
    out
}

fn file_row(conn: &Connection, path: &str) -> Result<Option<FoundFile>, String> {
    use rusqlite::OptionalExtension;
    conn.query_row("SELECT path, size, mtime FROM file_index WHERE path = ?1 AND dir = 0", [path], |row| {
        let path: String = row.get(0)?;
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        Ok(FoundFile { kind: crate::files::kind_of(&name).to_string(), name, size: row.get::<_, i64>(1)?.max(0) as u64, modified: row.get(2)?, snippet: None, path })
    })
    .optional()
    .map_err(|err| err.to_string())
}

/// The newest files that belong to `folder` itself (not to a described folder inside it).
fn newest(conn: &Connection, folder: &str, map: &Map, limit: i64) -> Result<Vec<FoundFile>, String> {
    let mut stmt = conn
        .prepare("SELECT path FROM file_index WHERE path >= ?1 || '/' AND path < ?1 || '0' AND dir = 0 ORDER BY mtime DESC LIMIT ?2")
        .map_err(|err| err.to_string())?;
    let paths: Vec<String> = stmt.query_map(params![folder, limit], |row| row.get::<_, String>(0)).map_err(|err| err.to_string())?.filter_map(Result::ok).collect();
    let mut out = Vec::new();
    for path in paths.into_iter().filter(|path| map.owner(path) == Some(folder)) {
        if let Some(file) = file_row(conn, &path)? {
            out.push(file);
        }
    }
    Ok(out)
}

fn snippet(conn: &Connection, path: &str, word: &str) -> Option<String> {
    let body: String = conn.query_row("SELECT body FROM content_text WHERE path = ?1", [path], |row| row.get(0)).ok()?;
    crate::files::content_snippet(&body, word)
}

// ---- Turning descriptions into vectors, in the background ----

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub working: bool,
    /// Descriptions Ask can use, and how many of them the current model has read.
    pub described: u64,
    pub ready: u64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

/// Reads new and changed descriptions with the current model, at the indexing speed.
#[derive(Default)]
pub struct AskIndex {
    status: Mutex<Status>,
    wake: AtomicBool,
}

/// The model's work on one text: (model, text, is it a question, threads) → its numbers.
pub type Embed = dyn Fn(&TextModel, &str, bool, usize) -> Result<Vec<f32>, String> + Send + Sync;

impl AskIndex {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|err| err.into_inner()).clone()
    }

    /// Descriptions changed, or the model did: look again now.
    pub fn wake(&self) {
        self.wake.store(true, Ordering::SeqCst);
    }

    /// `current` says which model to read with (none while Ask is off or its model is not
    /// downloaded yet).
    pub fn start(
        self: &Arc<Self>,
        db_path: PathBuf,
        speed: impl Fn() -> Speed + Send + 'static,
        current: impl Fn(&Connection) -> Option<&'static TextModel> + Send + 'static,
        embed: Arc<Embed>,
    ) {
        let index = self.clone();
        std::thread::Builder::new()
            .name("ownnas-ask-index".into())
            .spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(3600);
                loop {
                    if index.wake.swap(false, Ordering::SeqCst) || last.elapsed() >= Duration::from_secs(60) {
                        last = Instant::now();
                        if let Ok(conn) = crate::db::connect(&db_path) {
                            match current(&conn) {
                                Some(model) => {
                                    let result = index.run(&conn, model, &*embed, &speed);
                                    let mut status = index.status.lock().unwrap_or_else(|err| err.into_inner());
                                    status.working = false;
                                    status.error = result.err();
                                }
                                None => index.count(&conn, None),
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("could not start the Ask index thread");
    }

    fn count(&self, conn: &Connection, model: Option<&TextModel>) {
        let notes = folder_notes::all(conn).unwrap_or_default();
        let map = Map::new(&notes);
        let askable = askable(&notes, &map);
        let ready = match model {
            Some(model) => {
                let have: HashSet<String> = vectors(conn, model.id).map(|found| found.into_keys().collect()).unwrap_or_default();
                askable.iter().filter(|note| have.contains(&hash(&passage(&note.path, &note.description)))).count()
            }
            None => 0,
        };
        let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
        status.described = askable.len() as u64;
        status.ready = ready as u64;
    }

    /// Reads every description the model has not read yet, and forgets vectors no longer used
    /// (old texts, another model's).
    pub fn run(&self, conn: &Connection, model: &TextModel, embed: &Embed, speed: &dyn Fn() -> Speed) -> Result<(), String> {
        let notes = folder_notes::all(conn)?;
        let map = Map::new(&notes);
        let texts: Vec<String> = askable(&notes, &map).iter().map(|note| passage(&note.path, &note.description)).collect();
        let wanted: HashSet<String> = texts.iter().map(|text| hash(text)).collect();
        conn.execute("DELETE FROM text_vectors WHERE model != ?1", [model.id]).map_err(|err| err.to_string())?;
        let have: HashSet<String> = vectors(conn, model.id)?.into_keys().collect();
        for gone in have.difference(&wanted) {
            conn.execute("DELETE FROM text_vectors WHERE model = ?1 AND hash = ?2", params![model.id, gone]).map_err(|err| err.to_string())?;
        }
        let waiting: Vec<&String> = texts.iter().filter(|text| !have.contains(&hash(text))).collect();
        {
            let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            status.working = !waiting.is_empty();
            status.described = texts.len() as u64;
            status.ready = (texts.len() - waiting.len()) as u64;
            status.error = None;
        }
        let threads = if speed() == Speed::Fast { 2 } else { 1 };
        for text in waiting {
            let started = Instant::now();
            let vector = embed(model, text, false, threads)?;
            store_vector(conn, model.id, text, &vector)?;
            self.status.lock().unwrap_or_else(|err| err.into_inner()).ready += 1;
            let rest = speed().rest(started.elapsed());
            if !rest.is_zero() {
                std::thread::sleep(rest);
            }
            if self.wake.load(Ordering::SeqCst) {
                // Something changed meanwhile: the loop starts over with it.
                break;
            }
        }
        self.status.lock().unwrap_or_else(|err| err.into_inner()).finished_at = Some(crate::db::now_secs());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder_notes::set;

    struct Library {
        dir: PathBuf,
        root: PathBuf,
        conn: Connection,
    }

    impl Drop for Library {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl Library {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("ownnas-ask-{}-{}", std::process::id(), rand::random::<u32>()));
            let root = dir.join("lib");
            std::fs::create_dir_all(&root).unwrap();
            let conn = crate::db::open(&dir.join("ownnas.db")).unwrap();
            assert!(crate::text_index::ensure(&conn));
            Library { dir, root, conn }
        }

        fn folder(&self, path: &str, description: &str) {
            std::fs::create_dir_all(self.root.join(path)).unwrap();
            if !description.is_empty() {
                set(&self.conn, path, Some(description), None, "ana").unwrap();
            }
        }

        /// A file on disk, in the name index, and (with `text`) in the full-text index.
        fn file(&self, path: &str, text: &str, mtime: i64) {
            let full = self.root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, text).unwrap();
            let parent = path.rsplit_once('/').map(|(parent, _)| parent).unwrap_or("");
            let name = path.rsplit('/').next().unwrap().to_lowercase();
            self.conn
                .execute(
                    "INSERT INTO file_index(path, parent, name_lower, dir, size, mtime, kind) VALUES (?1, ?2, ?3, 0, ?4, ?5, 'text')",
                    params![path, parent, name, text.len() as i64, mtime],
                )
                .unwrap();
            if !text.is_empty() {
                self.conn.execute("INSERT INTO content_text(path, size, mtime, body, indexed_at) VALUES (?1, 1, 1, ?2, 1)", params![path, text]).unwrap();
                self.conn
                    .execute("INSERT INTO content_fts(rowid, lower, compact) VALUES (last_insert_rowid(), ?1, '')", params![text.to_lowercase()])
                    .unwrap();
            }
        }

        fn ask(&self, access: &Access, question: &str, meaning: Option<Meaning>) -> Answer {
            ask(&self.conn, &self.root, access, question, "", meaning).unwrap()
        }
    }

    fn paths(answer: &Answer) -> Vec<&str> {
        answer.folders.iter().map(|folder| folder.path.as_str()).collect()
    }

    /// A stand-in for the model: one number per topic it knows, in any language.
    fn fake_embed(_: &TextModel, text: &str, _: bool, _: usize) -> Result<Vec<f32>, String> {
        let text = fold(text);
        let topics = [&["myjni", "car wash", "lavado"][..], &["umow", "contract", "contrato"], &["faktur", "invoice", "factura"], &["urlop", "holiday", "vacacion"]];
        let mut vector: Vec<f32> = topics.iter().map(|words| if words.iter().any(|word| text.contains(word)) { 1.0 } else { 0.0 }).collect();
        vector.push(0.1);
        let length = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        vector.iter_mut().for_each(|v| *v /= length);
        Ok(vector)
    }

    fn library() -> Library {
        let library = Library::new();
        library.folder("Firma", "Dokumenty firmy");
        library.folder("Firma/Urząd/Myjnie", "Analiza wydawanych pozwoleń na budowę myjni w okresie 2019-2025");
        library.folder("Firma/Kadry", "Umowy o pracę, aneksy i świadectwa");
        library.folder("Firma/Księgowość", "Faktury kosztowe i sprzedażowe");
        library.folder("Firma/Kadry/Urlopy", "Wnioski urlopowe pracowników");
        library.folder("Prywatne", "Moje prywatne papiery");
        set(&library.conn, "Prywatne", None, Some(true), "ana").unwrap();
        library.file("Firma/Urząd/Myjnie/2021/Wnioski/decyzja-krakow.txt", "Decyzja o pozwoleniu na budowę myjni samochodowej w Krakowie", 100);
        library.file("Firma/Urząd/Myjnie/2021/zestawienie.txt", "", 300);
        library.file("Firma/Kadry/umowa-jan.txt", "Umowa o pracę Jan Kowalski", 50);
        library.file("Firma/Księgowość/faktura-01.txt", "Faktura VAT 01/2024", 60);
        library.file("Prywatne/myjnia.txt", "Rachunek za myjnię", 70);
        library
    }

    #[test]
    fn words_alone_find_folders_and_the_files_inside() {
        let library = library();
        let answer = library.ask(&Access::whole(), "Gdzie są pozwolenia na myjnie?", None);
        assert!(!answer.by_meaning);
        assert_eq!(paths(&answer), ["Firma/Urząd/Myjnie"], "only folders some word matches, without the model");
        let files: Vec<&str> = answer.folders[0].files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(files[0], "Firma/Urząd/Myjnie/2021/Wnioski/decyzja-krakow.txt", "the matching file first, from a working subfolder");
        assert_eq!(files.len(), 2, "then the newest of its own files");
        assert!(answer.folders[0].files[0].snippet.as_deref().unwrap().contains("myjni"));
    }

    #[test]
    fn meaning_finds_a_description_in_another_language() {
        let library = library();
        let index = AskIndex::default();
        index.run(&library.conn, &MULTILINGUAL, &fake_embed, &|| Speed::Fast).unwrap();
        assert_eq!(index.status().ready, 5, "every description but the left-out one");
        let question = "Where are the car wash permits?";
        let vector = fake_embed(&MULTILINGUAL, question, true, 1).unwrap();
        let answer = library.ask(&Access::whole(), question, Some(Meaning { model: MULTILINGUAL.id, vector: &vector }));
        assert!(answer.by_meaning);
        assert_eq!(answer.folders[0].path, "Firma/Urząd/Myjnie", "{:?}", paths(&answer));
        assert!((FEWEST_FOLDERS..=MOST_FOLDERS).contains(&answer.folders.len()));
        assert!(!paths(&answer).contains(&"Prywatne"), "left out of Ask");

        let question = "¿Dónde están los contratos de trabajo?";
        let vector = fake_embed(&MULTILINGUAL, question, true, 1).unwrap();
        let answer = library.ask(&Access::whole(), question, Some(Meaning { model: MULTILINGUAL.id, vector: &vector }));
        assert_eq!(answer.folders[0].path, "Firma/Kadry");
        assert_eq!(answer.folders[0].files[0].path, "Firma/Kadry/umowa-jan.txt", "its own file, not the described folder's inside it");
    }

    #[test]
    fn words_in_files_lift_a_folder_whose_description_misses_them() {
        let library = library();
        library.file("Firma/Księgowość/2023/Kraków/rachunek.txt", "Wynajem biura Kraków", 10);
        let answer = library.ask(&Access::whole(), "wynajem biura", None);
        assert_eq!(paths(&answer), ["Firma/Księgowość"], "the file's words count for the described folder above it");
    }

    #[test]
    fn answers_keep_to_what_the_account_may_see() {
        let library = library();
        let index = AskIndex::default();
        index.run(&library.conn, &MULTILINGUAL, &fake_embed, &|| Speed::Fast).unwrap();
        let limited = Access::limited_to(&[("Firma/Kadry", true)]);
        let question = "car wash permits";
        let vector = fake_embed(&MULTILINGUAL, question, true, 1).unwrap();
        let answer = library.ask(&limited, question, Some(Meaning { model: MULTILINGUAL.id, vector: &vector }));
        assert!(paths(&answer).iter().all(|path| path.starts_with("Firma/Kadry")), "{:?}", paths(&answer));
        assert!(!answer.folders.is_empty(), "a read-only folder can be asked about");
        let words = library.ask(&limited, "myjni pozwolenia", None);
        assert!(words.folders.is_empty(), "nothing outside its folders, not even by words");
        let none = Access::limited_to(&[]);
        assert!(library.ask(&none, question, None).folders.is_empty());
    }

    #[test]
    fn a_new_model_reads_everything_again() {
        let library = library();
        let index = AskIndex::default();
        index.run(&library.conn, &ENGLISH, &fake_embed, &|| Speed::Fast).unwrap();
        let count = |model: &str| -> i64 { library.conn.query_row("SELECT count(*) FROM text_vectors WHERE model = ?1", [model], |row| row.get(0)).unwrap() };
        assert_eq!(count(ENGLISH.id), 5);
        index.run(&library.conn, &MULTILINGUAL, &fake_embed, &|| Speed::Fast).unwrap();
        assert_eq!(count(ENGLISH.id), 0, "the old model's vectors are gone");
        assert_eq!(count(MULTILINGUAL.id), 5);
        // A changed description is read again; the old text's vector goes.
        set(&library.conn, "Firma/Kadry", Some("Contracts"), None, "ana").unwrap();
        index.run(&library.conn, &MULTILINGUAL, &fake_embed, &|| Speed::Fast).unwrap();
        assert_eq!(count(MULTILINGUAL.id), 5);
        // A moved folder keeps its description, and needs no new vector while its name and the
        // one above stay the same.
        std::fs::create_dir_all(library.root.join("Archiwum")).unwrap();
        std::fs::rename(library.root.join("Firma/Kadry"), library.root.join("Archiwum/Kadry")).unwrap();
        crate::db::rewrite_path_meta(&library.conn, "Firma/Kadry", "Archiwum/Kadry").unwrap();
        let vector = fake_embed(&MULTILINGUAL, "holiday", true, 1).unwrap();
        let answer = library.ask(&Access::whole(), "holiday", Some(Meaning { model: MULTILINGUAL.id, vector: &vector }));
        assert_eq!(answer.folders[0].path, "Archiwum/Kadry/Urlopy");
    }

    #[test]
    fn the_model_follows_the_languages_turned_on() {
        let pick = |codes: &[&str]| model_for(&codes.iter().map(|code| code.to_string()).collect::<Vec<_>>()).id;
        assert_eq!(pick(&["en"]), ENGLISH.id);
        assert_eq!(pick(&[]), ENGLISH.id);
        assert_eq!(pick(&["en", "pl", "es"]), MULTILINGUAL.id);
        assert_eq!(pick(&["pl"]), MULTILINGUAL.id);
        assert_eq!(parse_languages("en, PL,es,xx,pl"), ["en", "pl", "es"], "known codes, once each");
        assert_eq!(parse_languages(""), ["en"], "English by default");
        assert!(ENGLISH.size < 30_000_000 && MULTILINGUAL.size > 100_000_000);
    }

    #[test]
    fn question_words_are_cut_to_their_start() {
        let words: Vec<String> = keywords("Gdzie są pozwolenia na budowę myjni z 2021?").into_iter().map(|word| word.lower).collect();
        assert_eq!(words, ["pozwole", "budo", "myjn", "2021"]);
        assert_eq!(keywords("Łódź")[0].folded, "lodz");
    }
}
