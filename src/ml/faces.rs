//! Faces: YuNet finds them (a box and five points: eyes, nose, mouth corners), SFace turns each
//! one, straightened to a standard pose, into 128 numbers that are close for the same person.

use ort::session::Session;
use ort::value::Tensor;

/// YuNet looks at a 640×640 square.
const DETECT_SIZE: u32 = 640;
/// How sure YuNet must be that something is a face.
const MIN_SCORE: f32 = 0.82;
/// Overlapping boxes with more overlap than this are the same face.
const NMS_OVERLAP: f32 = 0.3;
/// Faces smaller than this (in pixels of the original photo) are too blurry to recognize.
const MIN_FACE: f32 = 36.0;
/// SFace reads a 112×112 face with its features at these points (eyes, nose, mouth corners).
const ALIGNED: u32 = 112;
const TEMPLATE: [(f32, f32); 5] = [(38.2946, 51.6963), (73.5318, 51.5014), (56.0252, 71.7366), (41.5493, 92.3655), (70.7299, 92.2041)];

#[derive(Clone, Debug)]
pub struct Face {
    /// The box, in pixels of the picture it was found in.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub score: f32,
    /// Right eye, left eye, nose tip, right and left mouth corners.
    pub landmarks: [(f32, f32); 5],
}

/// Finds the faces in a picture.
pub fn detect(session: &mut Session, image: &image::RgbImage) -> Result<Vec<Face>, String> {
    // Fit the picture into the square without stretching it; the rest stays black.
    let scale = DETECT_SIZE as f32 / image.width().max(image.height()) as f32;
    let (w, h) = (((image.width() as f32 * scale).round() as u32).max(1), ((image.height() as f32 * scale).round() as u32).max(1));
    let resized = image::imageops::resize(image, w.min(DETECT_SIZE), h.min(DETECT_SIZE), image::imageops::FilterType::Triangle);
    let plane = (DETECT_SIZE * DETECT_SIZE) as usize;
    let mut input = vec![0.0f32; plane * 3];
    for (x, y, pixel) in resized.enumerate_pixels() {
        let at = (y * DETECT_SIZE + x) as usize;
        // YuNet was trained on OpenCV's blue-green-red order, with 0–255 values.
        input[at] = pixel[2] as f32;
        input[plane + at] = pixel[1] as f32;
        input[2 * plane + at] = pixel[0] as f32;
    }
    let tensor = Tensor::from_array(([1usize, 3, DETECT_SIZE as usize, DETECT_SIZE as usize], input)).map_err(|err| err.to_string())?;
    let outputs = session.run(ort::inputs!["input" => tensor]).map_err(|err| err.to_string())?;
    let mut found = Vec::new();
    for stride in [8usize, 16, 32] {
        let read = |name: &str| -> Result<Vec<f32>, String> {
            let (_, values) = outputs[format!("{name}_{stride}").as_str()].try_extract_tensor::<f32>().map_err(|err| err.to_string())?;
            Ok(values.to_vec())
        };
        let (cls, obj, bbox, kps) = (read("cls")?, read("obj")?, read("bbox")?, read("kps")?);
        let cols = DETECT_SIZE as usize / stride;
        for index in 0..cls.len() {
            let score = (cls[index].clamp(0.0, 1.0) * obj[index].clamp(0.0, 1.0)).sqrt();
            if score < MIN_SCORE {
                continue;
            }
            let (row, col) = ((index / cols) as f32, (index % cols) as f32);
            let s = stride as f32;
            let cx = (col + bbox[index * 4]) * s;
            let cy = (row + bbox[index * 4 + 1]) * s;
            let bw = bbox[index * 4 + 2].exp() * s;
            let bh = bbox[index * 4 + 3].exp() * s;
            let mut landmarks = [(0.0, 0.0); 5];
            for (n, point) in landmarks.iter_mut().enumerate() {
                *point = (((kps[index * 10 + 2 * n] + col) * s) / scale, ((kps[index * 10 + 2 * n + 1] + row) * s) / scale);
            }
            found.push(Face { x: (cx - bw / 2.0) / scale, y: (cy - bh / 2.0) / scale, width: bw / scale, height: bh / scale, score, landmarks });
        }
    }
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::new();
    for face in found {
        if face.width.min(face.height) < MIN_FACE {
            continue;
        }
        if kept.iter().all(|other| overlap(&face, other) < NMS_OVERLAP) {
            kept.push(face);
        }
    }
    Ok(kept)
}

fn overlap(a: &Face, b: &Face) -> f32 {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.width).min(b.x + b.width);
    let y2 = (a.y + a.height).min(b.y + b.height);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = a.width * a.height + b.width * b.height - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// The 128 numbers for one face (length 1).
pub fn embed(session: &mut Session, image: &image::RgbImage, face: &Face) -> Result<Vec<f32>, String> {
    let aligned = align(image, &face.landmarks);
    let plane = (ALIGNED * ALIGNED) as usize;
    let mut input = vec![0.0f32; plane * 3];
    for (x, y, pixel) in aligned.enumerate_pixels() {
        let at = (y * ALIGNED + x) as usize;
        for channel in 0..3 {
            input[channel * plane + at] = pixel[channel] as f32;
        }
    }
    let tensor = Tensor::from_array(([1usize, 3, ALIGNED as usize, ALIGNED as usize], input)).map_err(|err| err.to_string())?;
    let outputs = session.run(ort::inputs!["data" => tensor]).map_err(|err| err.to_string())?;
    let (_, values) = outputs["fc1"].try_extract_tensor::<f32>().map_err(|err| err.to_string())?;
    let mut vector = values.to_vec();
    super::clip::normalize(&mut vector);
    Ok(vector)
}

/// The face turned, scaled, and moved so its features sit where SFace expects them.
pub fn align(image: &image::RgbImage, landmarks: &[(f32, f32); 5]) -> image::RgbImage {
    let (a, b, tx, ty) = similarity(landmarks, &TEMPLATE);
    // Map each output pixel back into the photo (the inverse of the transform) and sample there.
    let det = a * a + b * b;
    let (ia, ib) = (a / det, -b / det);
    let mut out = image::RgbImage::new(ALIGNED, ALIGNED);
    for (x, y, pixel) in out.enumerate_pixels_mut() {
        let (u, v) = (x as f32 - tx, y as f32 - ty);
        let sx = ia * u - ib * v;
        let sy = ib * u + ia * v;
        *pixel = sample(image, sx, sy);
    }
    out
}

/// The scale-and-rotation (a, b) and shift (tx, ty) that best move `from` onto `to`:
/// to = [a -b; b a] · from + (tx, ty).
fn similarity(from: &[(f32, f32); 5], to: &[(f32, f32); 5]) -> (f32, f32, f32, f32) {
    let n = from.len() as f32;
    let (fx, fy) = from.iter().fold((0.0, 0.0), |acc, p| (acc.0 + p.0 / n, acc.1 + p.1 / n));
    let (gx, gy) = to.iter().fold((0.0, 0.0), |acc, p| (acc.0 + p.0 / n, acc.1 + p.1 / n));
    let (mut dot, mut cross, mut norm) = (0.0f32, 0.0f32, 0.0f32);
    for (p, q) in from.iter().zip(to) {
        let (px, py, qx, qy) = (p.0 - fx, p.1 - fy, q.0 - gx, q.1 - gy);
        dot += px * qx + py * qy;
        cross += px * qy - py * qx;
        norm += px * px + py * py;
    }
    if norm <= f32::EPSILON {
        return (1.0, 0.0, gx - fx, gy - fy);
    }
    let (a, b) = (dot / norm, cross / norm);
    (a, b, gx - (a * fx - b * fy), gy - (b * fx + a * fy))
}

fn sample(image: &image::RgbImage, x: f32, y: f32) -> image::Rgb<u8> {
    let (w, h) = (image.width() as i64, image.height() as i64);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |px: i64, py: i64| -> [f32; 3] {
        if px < 0 || py < 0 || px >= w || py >= h {
            [0.0; 3]
        } else {
            let p = image.get_pixel(px as u32, py as u32);
            [p[0] as f32, p[1] as f32, p[2] as f32]
        }
    };
    let (c00, c10, c01, c11) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    let mut out = [0u8; 3];
    for i in 0..3 {
        let top = c00[i] * (1.0 - fx) + c10[i] * fx;
        let bottom = c01[i] * (1.0 - fx) + c11[i] * fx;
        out[i] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    image::Rgb(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_recovers_a_known_transform() {
        // Rotate by 30°, scale by 2, shift by (5, -3).
        let (angle, scale) = (30f32.to_radians(), 2.0f32);
        let (a, b) = (scale * angle.cos(), scale * angle.sin());
        let to = TEMPLATE;
        let mut from = [(0.0, 0.0); 5];
        // from = inverse transform of `to`.
        let det = a * a + b * b;
        for (i, q) in to.iter().enumerate() {
            let (u, v) = (q.0 - 5.0, q.1 + 3.0);
            from[i] = ((a * u + b * v) / det, (-b * u + a * v) / det);
        }
        let (ra, rb, tx, ty) = similarity(&from, &to);
        assert!((ra - a).abs() < 1e-3 && (rb - b).abs() < 1e-3, "{ra} {rb}");
        assert!((tx - 5.0).abs() < 1e-2 && (ty + 3.0).abs() < 1e-2, "{tx} {ty}");
    }
}
