//! On-device machine learning: describing photos so they can be searched by what they show
//! (CLIP), finding and recognizing faces (YuNet + SFace), and matching questions with folder
//! descriptions by meaning (Ask: MiniLM or multilingual e5). Everything runs on this computer;
//! the models are downloaded once, when a feature is switched on.

pub mod clip;
pub mod faces;
pub mod text;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ort::session::Session;
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Feature {
    /// Searching photos by description, and finding similar ones.
    Search,
    /// Finding faces and grouping them into people.
    Faces,
    /// Ask, with only English turned on (all-MiniLM-L6-v2).
    AskEnglish,
    /// Ask, with any other language turned on (multilingual-e5-small).
    AskMultilingual,
}

impl Feature {
    pub fn setting(self) -> &'static str {
        match self {
            Self::Search => "ml_search",
            Self::Faces => "ml_faces",
            Self::AskEnglish | Self::AskMultilingual => crate::folder_ask::SETTING_ON,
        }
    }

    /// The download behind one of Ask's models (`folder_ask::model_for`).
    pub fn for_text_model(model: &crate::folder_ask::TextModel) -> Self {
        if model.multilingual { Self::AskMultilingual } else { Self::AskEnglish }
    }

    fn files(self) -> &'static [ModelFile] {
        match self {
            Self::Search => &SEARCH_FILES,
            Self::Faces => &FACE_FILES,
            Self::AskEnglish => &ASK_ENGLISH_FILES,
            Self::AskMultilingual => &ASK_MULTILINGUAL_FILES,
        }
    }

    /// Bytes to download, for the setting's description.
    pub fn download_size(self) -> u64 {
        self.files().iter().map(|file| file.size).sum()
    }
}

struct ModelFile {
    name: &'static str,
    url: &'static str,
    sha256: &'static str,
    size: u64,
}

// CLIP ViT-B/32 (MIT License), as exported to ONNX and quantized by Xenova (Transformers.js).
static SEARCH_FILES: [ModelFile; 4] = [
    ModelFile { name: "clip-vision.onnx", url: "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/vision_model_quantized.onnx", sha256: "583fd1110a514667812fee7d684952aaf82a99b959760c8d7dca7e0ab9839299", size: 89_117_001 },
    ModelFile { name: "clip-text.onnx", url: "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/text_model_quantized.onnx", sha256: "73baab855d406190da9faa498cfedf65f15cf309f4cc7385b7b032e6d08e5c3a", size: 64_504_507 },
    ModelFile { name: "clip-vocab.json", url: "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/vocab.json", sha256: "5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363", size: 862_328 },
    ModelFile { name: "clip-merges.txt", url: "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/merges.txt", sha256: "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a", size: 524_619 },
];

// YuNet (face detection) and SFace (face recognition) from the OpenCV model zoo (Apache 2.0).
static FACE_FILES: [ModelFile; 2] = [
    ModelFile {
        name: "yunet.onnx",
        url: "https://github.com/opencv/opencv_zoo/raw/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx",
        sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
        size: 232_589,
    },
    ModelFile {
        name: "sface.onnx",
        url: "https://github.com/opencv/opencv_zoo/raw/main/models/face_recognition_sface/face_recognition_sface_2021dec.onnx",
        sha256: "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
        size: 38_696_353,
    },
];

// Ask's sentence models, pinned to one revision each and checked against these hashes. The sizes
// are what Settings shows (`folder_ask::TextModel`).
// all-MiniLM-L6-v2 (Apache 2.0), as exported to ONNX and quantized by Xenova (Transformers.js).
static ASK_ENGLISH_FILES: [ModelFile; 2] = [
    ModelFile {
        name: "minilm.onnx",
        url: "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/751bff37182d3f1213fa05d7196b954e230abad9/onnx/model_quantized.onnx",
        sha256: "afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1",
        size: 22_972_370,
    },
    ModelFile {
        name: "minilm-vocab.txt",
        url: "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/751bff37182d3f1213fa05d7196b954e230abad9/vocab.txt",
        sha256: "07eced375cec144d27c900241f3e339478dec958f92fddbc551f295c992038a3",
        size: 231_508,
    },
];
// multilingual-e5-small (MIT License), as exported to ONNX and quantized by Xenova.
static ASK_MULTILINGUAL_FILES: [ModelFile; 2] = [
    ModelFile {
        name: "e5-small.onnx",
        url: "https://huggingface.co/Xenova/multilingual-e5-small/resolve/761b726dd34fb83930e26aab4e9ac3899aa1fa78/onnx/model_quantized.onnx",
        sha256: "f80102d3f2a1229f387d3c81909990d8945513e347b0eab049f7de3c6f98c193",
        size: 118_308_185,
    },
    ModelFile {
        name: "e5-tokenizer.json",
        url: "https://huggingface.co/Xenova/multilingual-e5-small/resolve/761b726dd34fb83930e26aab4e9ac3899aa1fa78/tokenizer.json",
        sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
        size: 17_082_730,
    },
];

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    pub downloading: bool,
    pub done: u64,
    pub total: u64,
    pub error: Option<String>,
}

/// A loaded model, rebuilt when the number of threads it may use changes.
#[derive(Default)]
struct Slot {
    session: Option<Session>,
    threads: usize,
}

pub struct Models {
    dir: PathBuf,
    env: OnceLock<Result<ort::environment::Environment, String>>,
    downloads: Mutex<HashMap<Feature, Download>>,
    clip_vision: Mutex<Slot>,
    clip_text: Mutex<Slot>,
    tokenizer: OnceLock<Result<clip::Tokenizer, String>>,
    neutral: OnceLock<Vec<f32>>,
    yunet: Mutex<Slot>,
    sface: Mutex<Slot>,
    minilm: Mutex<Slot>,
    e5: Mutex<Slot>,
    wordpiece: Mutex<Option<std::sync::Arc<text::WordPiece>>>,
    unigram: Mutex<Option<std::sync::Arc<text::Unigram>>>,
}

impl Models {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("models"),
            env: OnceLock::new(),
            downloads: Mutex::default(),
            clip_vision: Mutex::default(),
            clip_text: Mutex::default(),
            tokenizer: OnceLock::new(),
            neutral: OnceLock::new(),
            yunet: Mutex::default(),
            sface: Mutex::default(),
            minilm: Mutex::default(),
            e5: Mutex::default(),
            wordpiece: Mutex::default(),
            unigram: Mutex::default(),
        }
    }

    /// Whether every file the feature needs is here.
    pub fn ready(&self, feature: Feature) -> bool {
        feature.files().iter().all(|file| {
            std::fs::metadata(self.dir.join(file.name)).map(|meta| meta.len() == file.size).unwrap_or(false)
        })
    }

    pub fn download_state(&self, feature: Feature) -> Download {
        self.downloads.lock().unwrap_or_else(|err| err.into_inner()).get(&feature).cloned().unwrap_or_default()
    }

    /// Fetches the feature's models in the background (each checked against its known hash);
    /// `done` runs when they are all here.
    pub fn download(self: &Arc<Self>, feature: Feature, done: impl FnOnce() + Send + 'static) {
        {
            let mut downloads = self.downloads.lock().unwrap_or_else(|err| err.into_inner());
            let state = downloads.entry(feature).or_default();
            if state.downloading {
                return;
            }
            *state = Download { downloading: true, done: 0, total: feature.download_size(), error: None };
        }
        let models = self.clone();
        std::thread::spawn(move || {
            let result = models.fetch_all(feature);
            let mut downloads = models.downloads.lock().unwrap_or_else(|err| err.into_inner());
            let state = downloads.entry(feature).or_default();
            state.downloading = false;
            match result {
                Ok(()) => {
                    state.done = state.total;
                    drop(downloads);
                    done();
                }
                Err(err) => state.error = Some(err),
            }
        });
    }

    fn fetch_all(&self, feature: Feature) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|_| "Could not create the models folder".to_string())?;
        let mut before = 0;
        for file in feature.files() {
            let dest = self.dir.join(file.name);
            if std::fs::metadata(&dest).map(|meta| meta.len() == file.size).unwrap_or(false) {
                before += file.size;
                self.progress(feature, before);
                continue;
            }
            self.fetch(feature, file, &dest, before)?;
            before += file.size;
        }
        Ok(())
    }

    fn progress(&self, feature: Feature, done: u64) {
        if let Some(state) = self.downloads.lock().unwrap_or_else(|err| err.into_inner()).get_mut(&feature) {
            state.done = done;
        }
    }

    fn fetch(&self, feature: Feature, file: &ModelFile, dest: &Path, before: u64) -> Result<(), String> {
        let response = ureq::get(file.url).call().map_err(|err| format!("Could not download {}: {err}", file.name))?;
        let mut reader = response.into_reader();
        let partial = dest.with_extension("part");
        let mut out = std::fs::File::create(&partial).map_err(|_| "Could not save the model".to_string())?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 256 * 1024];
        let mut got = 0u64;
        loop {
            let read = reader.read(&mut buffer).map_err(|err| format!("The download of {} stopped: {err}", file.name))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            out.write_all(&buffer[..read]).map_err(|_| "Could not save the model".to_string())?;
            got += read as u64;
            if got > file.size * 2 {
                return Err(format!("{} is larger than expected", file.name));
            }
            self.progress(feature, before + got);
        }
        out.flush().map_err(|_| "Could not save the model".to_string())?;
        drop(out);
        if hex::encode(hasher.finalize()) != file.sha256 {
            let _ = std::fs::remove_file(&partial);
            return Err(format!("{} did not match the expected file; it was not used", file.name));
        }
        std::fs::rename(&partial, dest).map_err(|_| "Could not save the model".to_string())
    }

    /// Deletes a feature's models (they are downloaded again if it is switched back on).
    pub fn remove(&self, feature: Feature) {
        for slot in match feature {
            Feature::Search => [&self.clip_vision, &self.clip_text],
            Feature::Faces => [&self.yunet, &self.sface],
            Feature::AskEnglish => [&self.minilm, &self.minilm],
            Feature::AskMultilingual => [&self.e5, &self.e5],
        } {
            *slot.lock().unwrap_or_else(|err| err.into_inner()) = Slot::default();
        }
        match feature {
            Feature::AskEnglish => *self.wordpiece.lock().unwrap_or_else(|err| err.into_inner()) = None,
            Feature::AskMultilingual => *self.unigram.lock().unwrap_or_else(|err| err.into_inner()) = None,
            _ => {}
        }
        for file in feature.files() {
            let _ = std::fs::remove_file(self.dir.join(file.name));
        }
    }

    fn env(&self) -> Result<&ort::environment::Environment, String> {
        self.env
            .get_or_init(|| ort::init().with_name("ownnas").build().map_err(|err| format!("Could not start ONNX Runtime: {err}")))
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Runs `work` with the model in `slot` (loaded from `file` with `threads` threads).
    fn with_session<T>(&self, slot: &Mutex<Slot>, file: &str, threads: usize, work: impl FnOnce(&mut Session) -> Result<T, String>) -> Result<T, String> {
        let mut slot = slot.lock().unwrap_or_else(|err| err.into_inner());
        if slot.session.is_none() || slot.threads != threads {
            let env = self.env()?;
            let fail = |err: &dyn std::fmt::Display| format!("Could not load {file}: {err}");
            let builder = Session::builder(env).map_err(|err| fail(&err))?;
            let builder = builder.with_intra_threads(threads).map_err(|err| fail(&err))?;
            let mut builder = builder.with_inter_threads(1).map_err(|err| fail(&err))?;
            let session = builder.commit_from_file(self.dir.join(file)).map_err(|err| fail(&err))?;
            *slot = Slot { session: Some(session), threads };
        }
        work(slot.session.as_mut().expect("loaded above"))
    }

    /// 512 numbers describing a picture.
    pub fn describe_image(&self, image: &image::DynamicImage, threads: usize) -> Result<Vec<f32>, String> {
        self.with_session(&self.clip_vision, "clip-vision.onnx", threads, |session| clip::embed_image(session, image))
    }

    /// 512 numbers for a phrase, comparable with pictures'.
    pub fn describe_text(&self, text: &str) -> Result<Vec<f32>, String> {
        let tokenizer = self
            .tokenizer
            .get_or_init(|| clip::Tokenizer::load(&self.dir.join("clip-vocab.json"), &self.dir.join("clip-merges.txt")))
            .as_ref()
            .map_err(Clone::clone)?;
        self.with_session(&self.clip_text, "clip-text.onnx", 2, |session| clip::embed_text(session, tokenizer, text))
    }

    /// The description of a neutral phrase, which searches are compared against.
    pub fn neutral_text(&self) -> Result<Vec<f32>, String> {
        if let Some(found) = self.neutral.get() {
            return Ok(found.clone());
        }
        let vector = self.describe_text("a photo")?;
        Ok(self.neutral.get_or_init(|| vector).clone())
    }

    /// 384 numbers for what a sentence means, with Ask's model for `feature`. e5 reads questions
    /// and the texts they look for differently, so it is told which this is.
    pub fn describe_sentence(&self, feature: Feature, text: &str, question: bool, threads: usize) -> Result<Vec<f32>, String> {
        match feature {
            Feature::AskEnglish => {
                let tokenizer = {
                    let mut held = self.wordpiece.lock().unwrap_or_else(|err| err.into_inner());
                    if held.is_none() {
                        *held = Some(Arc::new(text::WordPiece::load(&self.dir.join("minilm-vocab.txt"))?));
                    }
                    held.clone().expect("loaded above")
                };
                let ids = tokenizer.encode(text);
                self.with_session(&self.minilm, "minilm.onnx", threads, |session| text::embed(session, &ids))
            }
            Feature::AskMultilingual => {
                let tokenizer = {
                    let mut held = self.unigram.lock().unwrap_or_else(|err| err.into_inner());
                    if held.is_none() {
                        *held = Some(Arc::new(text::Unigram::load(&self.dir.join("e5-tokenizer.json"))?));
                    }
                    held.clone().expect("loaded above")
                };
                let marked = format!("{}{text}", if question { "query: " } else { "passage: " });
                let ids = tokenizer.encode(&marked);
                self.with_session(&self.e5, "e5-small.onnx", threads, |session| text::embed(session, &ids))
            }
            _ => Err("Not a sentence model".into()),
        }
    }

    /// The faces in a picture, each with its 128 numbers.
    pub fn find_faces(&self, image: &image::RgbImage, threads: usize) -> Result<Vec<(faces::Face, Vec<f32>)>, String> {
        let found = self.with_session(&self.yunet, "yunet.onnx", threads, |session| faces::detect(session, image))?;
        let mut out = Vec::with_capacity(found.len());
        for face in found {
            let vector = self.with_session(&self.sface, "sface.onnx", threads, |session| faces::embed(session, image, &face))?;
            out.push((face, vector));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smart::similarity;

    /// The downloaded models, when OWNNAS_TEST_MODELS points at a folder holding them
    /// (named as in the catalogue above).
    fn test_models() -> Option<Arc<Models>> {
        let dir = PathBuf::from(std::env::var("OWNNAS_TEST_MODELS").ok()?);
        let models = Models { dir: dir.clone(), ..Models::new(Path::new("/nonexistent")) };
        Some(Arc::new(models))
    }

    #[test]
    fn tokenizer_matches_clip() {
        let Some(models) = test_models() else { return };
        let tokenizer = clip::Tokenizer::load(&models.dir.join("clip-vocab.json"), &models.dir.join("clip-merges.txt")).unwrap();
        assert_eq!(tokenizer.encode("a photo of a cat"), vec![49406, 320, 1125, 539, 320, 2368, 49407]);
        assert_eq!(tokenizer.encode("  A   PHOTO of a CAT "), tokenizer.encode("a photo of a cat"));
        let long = tokenizer.encode(&"word ".repeat(200));
        assert_eq!(long.len(), 77);
        assert_eq!(*long.last().unwrap(), 49407);
    }

    #[test]
    fn faces_are_found_and_recognized() {
        let Some(models) = test_models() else { return };
        let Ok(dir) = std::env::var("OWNNAS_TEST_FACES") else { return };
        let dir = PathBuf::from(dir);
        let load = |name: &str| image::open(dir.join(name)).unwrap().to_rgb8();
        let faces = |name: &str| models.find_faces(&load(name), 2).unwrap();
        let lena = faces("lena.jpg");
        assert_eq!(lena.len(), 1, "one face in lena.jpg");
        let face = &lena[0].0;
        assert!(face.x > 150.0 && face.x < 300.0 && face.y > 150.0 && face.y < 300.0, "{face:?}");
        assert_eq!(lena[0].1.len(), 128);
        let other_person = faces("lincoln.jpg");
        assert_eq!(other_person.len(), 1, "one face in lincoln.jpg");
        let same = ["lena-flipped.jpg", "lena-crop.jpg", "lena-low.jpg"].map(|name| similarity(&lena[0].1, &faces(name)[0].1));
        let other = similarity(&lena[0].1, &other_person[0].1);
        println!("same person: {same:?}, different: {other}");
        assert!(same.iter().all(|&s| s > 0.6), "{same:?}");
        assert!(other < 0.3, "{other}");
    }

    /// With OWNNAS_TEST_MODELS holding Ask's models: questions find the description they are
    /// about, in the same language with the English model, across languages with e5.
    #[test]
    fn questions_meet_descriptions_across_languages() {
        let Some(models) = test_models() else { return };
        let descriptions = [
            "Urząd / Myjnie: Analiza wydawanych pozwoleń na budowę myjni w okresie 2019-2025",
            "Firma / Kadry: Umowy o pracę, aneksy i świadectwa pracy pracowników",
            "Firma / Księgowość: Faktury kosztowe i sprzedażowe, wyciągi bankowe",
            "Dom / Wakacje: Zdjęcia i rezerwacje z wyjazdów wakacyjnych",
        ];
        let best = |feature: Feature, question: &str, texts: &[&str]| -> usize {
            let asked = models.describe_sentence(feature, question, true, 2).unwrap();
            let scores: Vec<f32> = texts.iter().map(|text| similarity(&asked, &models.describe_sentence(feature, text, false, 2).unwrap())).collect();
            println!("{question}: {scores:?}");
            scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0
        };
        if models.ready(Feature::AskMultilingual) {
            assert_eq!(models.describe_sentence(Feature::AskMultilingual, "hello", true, 2).unwrap().len(), 384);
            for (question, expected) in [
                ("Where are the car wash building permits?", 0),
                ("¿Dónde están los permisos de construcción de lavaderos de coches?", 0),
                ("employment contracts of our staff", 1),
                ("¿Dónde guardamos las facturas?", 2),
                ("pozwolenia na myjnie", 0),
                ("holiday photos and bookings", 3),
            ] {
                assert_eq!(best(Feature::AskMultilingual, question, &descriptions), expected, "{question}");
            }
        }
        if models.ready(Feature::AskEnglish) {
            let english = ["Permits: Car wash building permits issued 2019-2025", "Staff: Employment contracts and references", "Accounts: Purchase and sales invoices, bank statements"];
            assert_eq!(best(Feature::AskEnglish, "where do we keep the invoices?", &english), 2);
            assert_eq!(best(Feature::AskEnglish, "permission to build a car wash", &english), 0);
            assert_eq!(best(Feature::AskEnglish, "job contract", &english), 1);
        }
    }

    #[test]
    fn pictures_and_phrases_meet() {
        let Some(models) = test_models() else { return };
        let car = image::open(std::env::var("OWNNAS_TEST_CAR").expect("OWNNAS_TEST_CAR: a photo of a car")).unwrap();
        let picture = models.describe_image(&car, 2).unwrap();
        assert_eq!(picture.len(), 512);
        let car_text = models.describe_text("a photo of a car").unwrap();
        let dog_text = models.describe_text("a photo of a dog").unwrap();
        let (with_car, with_dog) = (similarity(&picture, &car_text), similarity(&picture, &dog_text));
        assert!(with_car > with_dog + 0.03, "car {with_car} vs dog {with_dog}");
    }
}
