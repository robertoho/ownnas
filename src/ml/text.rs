//! Sentence embeddings for Ask: a folder's description and a question each become 384 numbers,
//! close together when they mean similar things. Two models, chosen by the languages turned on
//! (`folder_ask::model_for`): all-MiniLM-L6-v2 for English alone, multilingual-e5-small when any
//! other language is on (it puts a question in one language next to a description in another).
//!
//! Each model reads text through its own tokenizer, written out here so no other library is
//! needed: WordPiece (BERT's, lower case) for MiniLM, and SentencePiece's unigram model, as
//! XLM-RoBERTa uses it, for e5.

use std::collections::HashMap;
use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;
use unicode_normalization::UnicodeNormalization;

/// Tokens read at most, start and end markers included. Descriptions and questions are short;
/// a longer text is cut, which keeps each one quick on a small processor.
const MAX_TOKENS: usize = 256;

/// BERT's WordPiece, as all-MiniLM-L6-v2 uses it (lower case, accents removed).
pub struct WordPiece {
    vocab: HashMap<String, i64>,
    unknown: i64,
    start: i64,
    end: i64,
}

impl WordPiece {
    pub fn load(vocab: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(vocab).map_err(|err| format!("Could not read the vocabulary: {err}"))?;
        Ok(Self::from_lines(&text))
    }

    pub fn from_lines(text: &str) -> Self {
        let vocab: HashMap<String, i64> = text.lines().enumerate().map(|(id, piece)| (piece.to_string(), id as i64)).collect();
        let id = |piece: &str, default: i64| vocab.get(piece).copied().unwrap_or(default);
        let (unknown, start, end) = (id("[UNK]", 100), id("[CLS]", 101), id("[SEP]", 102));
        Self { vocab, unknown, start, end }
    }

    pub fn encode(&self, text: &str) -> Vec<i64> {
        let mut ids = vec![self.start];
        for word in basic_words(text) {
            ids.extend(self.pieces(&word));
            if ids.len() >= MAX_TOKENS - 1 {
                break;
            }
        }
        ids.truncate(MAX_TOKENS - 1);
        ids.push(self.end);
        ids
    }

    /// The longest pieces from the start of the word that are in the vocabulary, the ones after
    /// the first marked "##"; a word that cannot be split this way is one unknown token.
    fn pieces(&self, word: &str) -> Vec<i64> {
        let chars: Vec<char> = word.chars().collect();
        if chars.len() > 100 {
            return vec![self.unknown];
        }
        let mut out = Vec::new();
        let mut start = 0;
        while start < chars.len() {
            let mut end = chars.len();
            let mut found = None;
            while end > start {
                let piece: String = chars[start..end].iter().collect();
                let piece = if start > 0 { format!("##{piece}") } else { piece };
                if let Some(&id) = self.vocab.get(&piece) {
                    found = Some(id);
                    break;
                }
                end -= 1;
            }
            let Some(id) = found else { return vec![self.unknown] };
            out.push(id);
            start = end;
        }
        out
    }
}

/// BERT's first step: control characters dropped, lower case without accents, and split at
/// spaces and punctuation (each punctuation mark, and each Chinese character, a word of its own).
fn basic_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let lower: String = text.to_lowercase().nfd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).collect();
    for c in lower.chars() {
        if c.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else if c.is_control() || c == '\u{fffd}' {
            continue;
        } else if is_punctuation(c) || is_cjk(c) {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            words.push(c.to_string());
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace() && matches!(c, '\u{2000}'..='\u{206f}' | '\u{3000}'..='\u{303f}' | '¡' | '¿' | '«' | '»' | '§' | '¶' | '·'))
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0x2A700..=0x2CEAF | 0xF900..=0xFAFF | 0x2F800..=0x2FA1F)
}

/// SentencePiece's unigram model as XLM-RoBERTa (and so multilingual-e5) uses it: the text made
/// uniform (NFKC, single spaces), each word starting with "▁", and each word split into the
/// pieces whose scores add up highest.
pub struct Unigram {
    pieces: HashMap<String, (i64, f32)>,
    /// The longest piece, in characters.
    longest: usize,
    unknown: i64,
    unknown_score: f32,
}

const START_ID: i64 = 0;
const END_ID: i64 = 2;
const SPACE: char = '\u{2581}';

#[derive(serde::Deserialize)]
struct TokenizerFile {
    model: UnigramFile,
}

#[derive(serde::Deserialize)]
struct UnigramFile {
    vocab: Vec<(String, f64)>,
    unk_id: Option<usize>,
}

impl Unigram {
    /// From a Hugging Face `tokenizer.json`.
    pub fn load(file: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(file).map_err(|err| format!("Could not read the tokenizer: {err}"))?;
        let parsed: TokenizerFile = serde_json::from_slice(&bytes).map_err(|err| format!("Could not read the tokenizer: {err}"))?;
        Ok(Self::from_vocab(parsed.model.vocab, parsed.model.unk_id.unwrap_or(3)))
    }

    pub fn from_vocab(vocab: Vec<(String, f64)>, unknown: usize) -> Self {
        let lowest = vocab.iter().map(|(_, score)| *score as f32).fold(0.0f32, f32::min);
        let mut longest = 1;
        let mut pieces = HashMap::with_capacity(vocab.len());
        for (id, (piece, score)) in vocab.into_iter().enumerate() {
            // The markers (<s>, <pad>, </s>, <unk>, <mask>) are never read from the text.
            if piece.starts_with('<') && piece.ends_with('>') && piece.len() > 2 {
                continue;
            }
            longest = longest.max(piece.chars().count());
            pieces.insert(piece, (id as i64, score as f32));
        }
        Self { pieces, longest, unknown: unknown as i64, unknown_score: lowest - 10.0 }
    }

    pub fn encode(&self, text: &str) -> Vec<i64> {
        let normalized = normalize_nfkc(text);
        let mut ids = vec![START_ID];
        if !normalized.is_empty() {
            let marked: String = std::iter::once(SPACE).chain(normalized.chars().map(|c| if c == ' ' { SPACE } else { c })).collect();
            let mut word = String::new();
            for c in marked.chars() {
                if c == SPACE && !word.is_empty() {
                    ids.extend(self.best_split(&word));
                    word.clear();
                }
                word.push(c);
                if ids.len() >= MAX_TOKENS {
                    break;
                }
            }
            if !word.is_empty() {
                ids.extend(self.best_split(&word));
            }
        }
        ids.truncate(MAX_TOKENS - 1);
        ids.push(END_ID);
        ids
    }

    /// Viterbi: the split of `word` into known pieces with the highest total score. A character
    /// no piece covers is unknown, and unknown characters in a row are one unknown token.
    fn best_split(&self, word: &str) -> Vec<i64> {
        let chars: Vec<char> = word.chars().collect();
        let n = chars.len();
        // best[i]: the score of the best split of the first i characters, and its last piece.
        let mut best: Vec<Option<(f32, usize, i64)>> = vec![None; n + 1];
        best[0] = Some((0.0, 0, -1));
        for start in 0..n {
            let Some((score, _, _)) = best[start] else { continue };
            let mut single = false;
            let mut piece = String::new();
            for end in start + 1..=(start + self.longest).min(n) {
                piece.push(chars[end - 1]);
                if let Some(&(id, piece_score)) = self.pieces.get(&piece) {
                    single |= end == start + 1;
                    let total = score + piece_score;
                    if best[end].is_none_or(|(known, _, _)| total > known) {
                        best[end] = Some((total, start, id));
                    }
                }
            }
            if !single {
                let total = score + self.unknown_score;
                if best[start + 1].is_none_or(|(known, _, _)| total > known) {
                    best[start + 1] = Some((total, start, self.unknown));
                }
            }
        }
        let mut out = Vec::new();
        let mut at = n;
        while at > 0 {
            let Some((_, from, id)) = best[at] else { break };
            if !(id == self.unknown && out.last() == Some(&self.unknown)) {
                out.push(id);
            }
            at = from;
        }
        out.reverse();
        out
    }
}

/// Close to SentencePiece's own normalization ("nmt_nfkc"): compatibility forms folded (NFKC),
/// control characters dropped, every kind of space a plain one, never two in a row, none at
/// either end.
fn normalize_nfkc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for c in text.nfkc() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else if c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{feff}') {
            continue;
        } else {
            out.push(c);
            space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// The text's meaning: the model's numbers for each token, averaged and made length 1.
pub fn embed(session: &mut Session, ids: &[i64]) -> Result<Vec<f32>, String> {
    let n = ids.len();
    let names: Vec<String> = session.inputs().iter().map(|input| input.name().to_string()).collect();
    let output = session.outputs().first().map(|output| output.name().to_string()).ok_or("The model has no output")?;
    let mut inputs = Vec::new();
    for name in names {
        let values = match name.as_str() {
            "input_ids" => ids.to_vec(),
            "attention_mask" => vec![1i64; n],
            "token_type_ids" => vec![0i64; n],
            other => return Err(format!("The model asks for {other}, which OwnNAS does not know")),
        };
        inputs.push((name, Tensor::from_array(([1usize, n], values)).map_err(|err| err.to_string())?));
    }
    let outputs = session.run(inputs).map_err(|err| err.to_string())?;
    let (shape, values) = outputs[output.as_str()].try_extract_tensor::<f32>().map_err(|err| err.to_string())?;
    let dims = shape.last().copied().unwrap_or(0).max(0) as usize;
    if dims == 0 || values.len() < dims {
        return Err("The model gave nothing back".into());
    }
    let tokens = values.len() / dims;
    let mut vector = vec![0.0f32; dims];
    for token in 0..tokens {
        for (at, value) in vector.iter_mut().enumerate() {
            *value += values[token * dims + at];
        }
    }
    vector.iter_mut().for_each(|value| *value /= tokens as f32);
    super::clip::normalize(&mut vector);
    Ok(vector)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wordpiece_splits_like_bert() {
        let vocab = "[PAD]\n[UNK]\n[CLS]\n[SEP]\nhello\nworld\nun\n##aff\n##able\n,\ncafe\n!";
        let pieces = WordPiece::from_lines(vocab);
        assert_eq!(pieces.encode("Hello, World!"), vec![2, 4, 9, 5, 11, 3]);
        assert_eq!(pieces.encode("unaffable"), vec![2, 6, 7, 8, 3]);
        assert_eq!(pieces.encode("Café"), vec![2, 10, 3], "lower case without accents");
        assert_eq!(pieces.encode("zzz"), vec![2, 1, 3], "unknown words");
    }

    #[test]
    fn unigram_picks_the_best_scoring_split() {
        let vocab = vec![
            ("<s>".to_string(), 0.0),
            ("<pad>".to_string(), 0.0),
            ("</s>".to_string(), 0.0),
            ("<unk>".to_string(), 0.0),
            ("▁".to_string(), -2.0),
            ("▁my".to_string(), -3.0),
            ("jni".to_string(), -4.0),
            ("▁myjni".to_string(), -5.0),
            ("a".to_string(), -2.0),
            ("▁a".to_string(), -2.5),
            ("▁b".to_string(), -2.5),
        ];
        let model = Unigram::from_vocab(vocab, 3);
        // "▁myjni" alone (-5) beats "▁my" + "jni" (-7).
        assert_eq!(model.encode("myjnia"), vec![0, 7, 8, 2]);
        assert_eq!(model.encode("  a\u{3000}b "), vec![0, 9, 10, 2], "spaces made uniform");
        assert_eq!(model.encode("żż a"), vec![0, 4, 3, 9, 2], "unknown characters in a row are one token");
        assert_eq!(model.encode(""), vec![0, 2]);
    }
}
