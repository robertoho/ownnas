//! CLIP: one model for pictures and words. Pictures and phrases become 512 numbers each, close
//! together when they show (or say) similar things, which is how "find similar" and searching
//! photos by description work.

use std::collections::HashMap;
use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;

/// The model sees a 224×224 square.
const SIZE: u32 = 224;
const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];
const START: i64 = 49406;
const END: i64 = 49407;
/// Words (tokens) the text model reads at most, start and end included.
const MAX_TOKENS: usize = 77;

/// Makes `vector` length 1, so comparing two is a plain dot product.
pub fn normalize(vector: &mut [f32]) {
    let length = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if length > 0.0 {
        vector.iter_mut().for_each(|v| *v /= length);
    }
}

/// The picture as the model wants it: shortest side 224, centered square, scaled colors.
pub fn pixels(image: &image::DynamicImage) -> Vec<f32> {
    let (width, height) = (image.width().max(1), image.height().max(1));
    let scale = SIZE as f32 / width.min(height) as f32;
    let (w, h) = (((width as f32 * scale).round() as u32).max(SIZE), ((height as f32 * scale).round() as u32).max(SIZE));
    let resized = image.resize_exact(w, h, image::imageops::FilterType::CatmullRom).to_rgb8();
    let (left, top) = ((w - SIZE) / 2, (h - SIZE) / 2);
    let plane = (SIZE * SIZE) as usize;
    let mut out = vec![0.0f32; plane * 3];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let pixel = resized.get_pixel(left + x, top + y);
            let at = (y * SIZE + x) as usize;
            for channel in 0..3 {
                out[channel * plane + at] = (pixel[channel] as f32 / 255.0 - MEAN[channel]) / STD[channel];
            }
        }
    }
    out
}

pub fn embed_image(session: &mut Session, image: &image::DynamicImage) -> Result<Vec<f32>, String> {
    let input = Tensor::from_array(([1usize, 3, SIZE as usize, SIZE as usize], pixels(image))).map_err(|err| err.to_string())?;
    let outputs = session.run(ort::inputs!["pixel_values" => input]).map_err(|err| err.to_string())?;
    let (_, values) = outputs["image_embeds"].try_extract_tensor::<f32>().map_err(|err| err.to_string())?;
    let mut vector = values.to_vec();
    normalize(&mut vector);
    Ok(vector)
}

pub fn embed_text(session: &mut Session, tokenizer: &Tokenizer, text: &str) -> Result<Vec<f32>, String> {
    let ids = tokenizer.encode(text);
    let input = Tensor::from_array(([1usize, ids.len()], ids)).map_err(|err| err.to_string())?;
    let outputs = session.run(ort::inputs!["input_ids" => input]).map_err(|err| err.to_string())?;
    let (_, values) = outputs["text_embeds"].try_extract_tensor::<f32>().map_err(|err| err.to_string())?;
    let mut vector = values.to_vec();
    normalize(&mut vector);
    Ok(vector)
}

/// CLIP's byte-pair tokenizer: lowercase words split into pieces from its 49,408-piece vocabulary.
pub struct Tokenizer {
    vocab: HashMap<String, i64>,
    ranks: HashMap<(String, String), usize>,
    bytes: [char; 256],
    pattern: regex::Regex,
}

impl Tokenizer {
    pub fn load(vocab: &Path, merges: &Path) -> Result<Self, String> {
        let vocab: HashMap<String, i64> = serde_json::from_slice(&std::fs::read(vocab).map_err(|err| err.to_string())?)
            .map_err(|err| format!("Could not read the vocabulary: {err}"))?;
        let merges = std::fs::read_to_string(merges).map_err(|err| err.to_string())?;
        let ranks = merges
            .lines()
            .filter(|line| !line.starts_with("#version") && !line.trim().is_empty())
            .filter_map(|line| line.split_once(' ').map(|(a, b)| (a.to_string(), b.to_string())))
            .enumerate()
            .map(|(rank, pair)| (pair, rank))
            .collect();
        let pattern = regex::Regex::new(r"(?i)<\|startoftext\|>|<\|endoftext\|>|'s|'t|'re|'ve|'m|'ll|'d|\p{L}+|\p{N}|[^\s\p{L}\p{N}]+")
            .map_err(|err| err.to_string())?;
        Ok(Self { vocab, ranks, bytes: byte_chars(), pattern })
    }

    /// Token ids for a phrase, with the start and end markers, at most 77.
    pub fn encode(&self, text: &str) -> Vec<i64> {
        let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let mut ids = vec![START];
        for word in self.pattern.find_iter(&cleaned) {
            let mapped: String = word.as_str().bytes().map(|byte| self.bytes[byte as usize]).collect();
            for piece in self.bpe(&mapped) {
                if let Some(&id) = self.vocab.get(&piece) {
                    ids.push(id);
                }
            }
        }
        ids.truncate(MAX_TOKENS - 1);
        ids.push(END);
        ids
    }

    /// Splits a word into characters (the last marked as the word's end) and joins the pair
    /// that comes first in the merge list until none is left.
    fn bpe(&self, word: &str) -> Vec<String> {
        let chars: Vec<char> = word.chars().collect();
        let Some((last, rest)) = chars.split_last() else { return Vec::new() };
        let mut parts: Vec<String> = rest.iter().map(|c| c.to_string()).collect();
        parts.push(format!("{last}</w>"));
        while parts.len() > 1 {
            let best = parts
                .windows(2)
                .enumerate()
                .filter_map(|(at, pair)| self.ranks.get(&(pair[0].clone(), pair[1].clone())).map(|rank| (rank, at)))
                .min();
            let Some((_, at)) = best else { break };
            let joined = format!("{}{}", parts[at], parts[at + 1]);
            parts.splice(at..at + 2, [joined]);
        }
        parts
    }
}

/// GPT-2's map from bytes to printable characters, so any text can be split into pieces.
fn byte_chars() -> [char; 256] {
    let mut table = ['\0'; 256];
    let printable: Vec<u32> = (b'!' as u32..=b'~' as u32).chain(0xA1..=0xAC).chain(0xAE..=0xFF).collect();
    let mut extra = 0u32;
    for byte in 0..256u32 {
        table[byte as usize] = if printable.contains(&byte) {
            char::from_u32(byte).unwrap_or('?')
        } else {
            extra += 1;
            char::from_u32(255 + extra).unwrap_or('?')
        };
    }
    table
}
