//! On-device machine learning for the Gallery: describing photos so they can be searched by
//! what they show (CLIP), and finding and recognizing faces (YuNet + SFace). Everything runs on
//! this computer; the models are downloaded once, when a feature is switched on.

pub mod clip;
pub mod faces;

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
}

impl Feature {
    pub fn setting(self) -> &'static str {
        match self {
            Self::Search => "ml_search",
            Self::Faces => "ml_faces",
        }
    }

    fn files(self) -> &'static [ModelFile] {
        match self {
            Self::Search => &SEARCH_FILES,
            Self::Faces => &FACE_FILES,
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
        } {
            *slot.lock().unwrap_or_else(|err| err.into_inner()) = Slot::default();
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
