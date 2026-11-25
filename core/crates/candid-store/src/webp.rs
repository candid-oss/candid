//! Screenshot originals are stored as lossy WebP (data model v1 §9.1, quality about 60). macOS can read WebP but
//! has no writer, so the core links libwebp statically and encodes here; the App hands raw pixels to
//! `candid encode-webp` at capture time, after OCR has seen the uncompressed frame.
use libwebp_sys as sys;

/// Data model v1 §9.1: measured on real screenshots, about a third of the old JPEG with no OCR recall loss.
pub const DEFAULT_QUALITY: f32 = 60.0;
/// WebP's own limit is 16383 per edge.
pub const MAX_EDGE: u32 = 16_383;

/// Encodes 8-bit BGRA (or BGRX) pixels, rows `stride` bytes apart, as lossy WebP. Alpha is ignored: screenshots are
/// opaque, and dropping it keeps the file smaller.
pub fn encode_bgra(pixels: &[u8], width: u32, height: u32, stride: usize, quality: f32) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        return Err(format!("图片尺寸无效：{width}×{height}"));
    }
    if stride < width as usize * 4 || stride > i32::MAX as usize {
        return Err("每行字节数无效".into());
    }
    let needed = stride.checked_mul(height as usize - 1).and_then(|n| n.checked_add(width as usize * 4));
    if needed.is_none_or(|n| pixels.len() < n) {
        return Err("像素数据不完整".into());
    }
    if !quality.is_finite() || !(0.0..=100.0).contains(&quality) {
        return Err("质量必须在 0 到 100 之间".into());
    }
    unsafe {
        let mut config: sys::WebPConfig = std::mem::zeroed();
        if sys::WebPConfigInitInternal(&mut config, sys::WebPPreset::WEBP_PRESET_DEFAULT, quality, sys::WEBP_ENCODER_ABI_VERSION as i32) == 0 {
            return Err("WebP 编码器版本不符".into());
        }
        // Method 4 is libwebp's default balance; method 6 is a few percent smaller but about twice as slow.
        config.method = 4;
        config.alpha_compression = 0;
        if sys::WebPValidateConfig(&config) == 0 {
            return Err("WebP 参数无效".into());
        }
        let mut picture: sys::WebPPicture = std::mem::zeroed();
        if sys::WebPPictureInitInternal(&mut picture, sys::WEBP_ENCODER_ABI_VERSION as i32) == 0 {
            return Err("WebP 编码器版本不符".into());
        }
        picture.width = width as i32;
        picture.height = height as i32;
        // BGRX import: no alpha plane is kept.
        if sys::WebPPictureImportBGRX(&mut picture, pixels.as_ptr(), stride as i32) == 0 {
            sys::WebPPictureFree(&mut picture);
            return Err("WebP 内存不足".into());
        }
        let mut writer: sys::WebPMemoryWriter = std::mem::zeroed();
        sys::WebPMemoryWriterInit(&mut writer);
        picture.writer = Some(sys::WebPMemoryWrite);
        picture.custom_ptr = (&mut writer as *mut sys::WebPMemoryWriter).cast();
        let ok = sys::WebPEncode(&config, &mut picture);
        let code = picture.error_code;
        sys::WebPPictureFree(&mut picture);
        let out = if ok != 0 && !writer.mem.is_null() { std::slice::from_raw_parts(writer.mem, writer.size).to_vec() } else { Vec::new() };
        sys::WebPMemoryWriterClear(&mut writer);
        if ok == 0 || out.is_empty() {
            return Err(format!("WebP 编码失败（{code:?}）"));
        }
        Ok(out)
    }
}

/// `RIFF....WEBP`.
pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// Image bytes and MIME type to send to a model. A model server on this Mac may not accept WebP (LM Studio rejects
/// it with HTTP 400), so for a local one stored WebP screenshots go out as PNG; a remote service gets the WebP, about a
/// tenth of the PNG's size (measured: 3.4 s against 10.9 s per call). Anything else is unchanged.
pub fn for_model(bytes: Vec<u8>, mime: &str, local: bool) -> (Vec<u8>, String) {
    if local && is_webp(&bytes) {
        if let Some(png) = png_from_webp(&bytes) {
            return (png, "image/png".into());
        }
    }
    (bytes, mime.to_string())
}

/// (width, height, RGB bytes row by row) of a WebP image.
pub fn decode_rgb(webp: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let (mut w, mut h) = (0, 0);
    // SAFETY: libwebp reads `webp` within its length and returns one RGB allocation of w*h*3 bytes, or null.
    let rgb = unsafe { sys::WebPDecodeRGB(webp.as_ptr(), webp.len(), &mut w, &mut h) };
    if rgb.is_null() {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    // SAFETY: the allocation holds h rows of w*3 bytes; it is copied, then freed once.
    let pixels = unsafe { std::slice::from_raw_parts(rgb, w * h * 3) }.to_vec();
    unsafe { sys::WebPFree(rgb as *mut _) };
    Some((w, h, pixels))
}

fn png_from_webp(webp: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write;
    let (w, h, pixels) = decode_rgb(webp)?;
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for row in pixels.chunks_exact(w * 3) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&raw).ok()?;
    let idat = z.finish().ok()?;
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend(kind);
        out.extend(data);
        out.extend(crc.sum().to_be_bytes());
    }
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend((w as u32).to_be_bytes());
    ihdr.extend((h as u32).to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]); // 8-bit RGB, deflate, no filter method, no interlace
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

/// Thumbnail edge sizes for change detection (`luma_thumbnail`).
pub const THUMB_W: usize = 96;
pub const THUMB_H: usize = 60;

/// A tiny grayscale thumbnail (`THUMB_W` × `THUMB_H`, box-averaged luma) of a WebP image, for telling whether two
/// screenshots of a window differ without OCR. `None` when the bytes do not decode.
pub fn luma_thumbnail(webp: &[u8]) -> Option<Vec<u8>> {
    let (mut w, mut h, mut stride, mut uv_stride) = (0, 0, 0, 0);
    let (mut u, mut v) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: libwebp reads `webp` within its length and returns one allocation (Y, then U and V) or null.
    let y = unsafe { sys::WebPDecodeYUV(webp.as_ptr(), webp.len(), &mut w, &mut h, &mut u, &mut v, &mut stride, &mut uv_stride) };
    if y.is_null() {
        return None;
    }
    let (w, h, stride) = (w as usize, h as usize, stride as usize);
    let mut out = None;
    if w >= THUMB_W && h >= THUMB_H && stride >= w {
        // SAFETY: the Y plane holds `h` rows of `stride` bytes.
        let plane = unsafe { std::slice::from_raw_parts(y, stride * (h - 1) + w) };
        let mut thumb = vec![0u8; THUMB_W * THUMB_H];
        for ty in 0..THUMB_H {
            let (y0, y1) = (ty * h / THUMB_H, ((ty + 1) * h / THUMB_H).max(ty * h / THUMB_H + 1));
            for tx in 0..THUMB_W {
                let (x0, x1) = (tx * w / THUMB_W, ((tx + 1) * w / THUMB_W).max(tx * w / THUMB_W + 1));
                // Sample every other pixel each way; plenty for an average.
                let (mut sum, mut n) = (0u32, 0u32);
                for yy in (y0..y1).step_by(2) {
                    for xx in (x0..x1).step_by(2) {
                        sum += plane[yy * stride + xx] as u32;
                        n += 1;
                    }
                }
                thumb[ty * THUMB_W + tx] = (sum / n.max(1)) as u8;
            }
        }
        out = Some(thumb);
    }
    // SAFETY: `y` came from WebPDecodeYUV and is freed once; U and V live in the same allocation.
    unsafe { sys::WebPFree(y as *mut _) };
    out
}

/// Share of thumbnail cells whose brightness changed noticeably between two `luma_thumbnail`s, 0–1.
pub fn thumbnail_change(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 1.0;
    }
    a.iter().zip(b).filter(|(x, y)| x.abs_diff(**y) > 12).count() as f64 / a.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, stride: usize) -> Vec<u8> {
        let mut p = vec![0u8; stride * height as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let i = y * stride + x * 4;
                // A text-like pattern: dark strokes on white, so the size is in a realistic range.
                let ink = (x / 3 + y / 7) % 11 == 0 || (y % 18 < 2 && x % 200 < 150);
                let v = if ink { 30 } else { 245 };
                p[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        p
    }

    #[test]
    fn encodes_padded_rows_as_webp() {
        let (w, h, stride) = (321u32, 97u32, 321 * 4 + 12);
        let out = encode_bgra(&frame(w, h, stride), w, h, stride, DEFAULT_QUALITY).unwrap();
        assert!(is_webp(&out));
        // VP8 (lossy) bitstream, not VP8L/VP8X: no alpha chunk is written.
        assert_eq!(&out[12..16], b"VP8 ");
        let (mut ww, mut hh) = (0, 0);
        assert_ne!(unsafe { sys::WebPGetInfo(out.as_ptr(), out.len(), &mut ww, &mut hh) }, 0);
        assert_eq!((ww, hh), (w as i32, h as i32));
    }

    #[test]
    fn thumbnails_see_changed_regions() {
        let (w, h, stride) = (640u32, 400u32, 640 * 4);
        let base = frame(w, h, stride);
        let mut changed = base.clone();
        // Paint a bright block over the lower part, like a new chat bubble.
        for y in 300..360 {
            for x in 360..600 {
                let i = y * stride + x * 4;
                changed[i..i + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let a = luma_thumbnail(&encode_bgra(&base, w, h, stride, DEFAULT_QUALITY).unwrap()).unwrap();
        let a2 = luma_thumbnail(&encode_bgra(&base, w, h, stride, DEFAULT_QUALITY).unwrap()).unwrap();
        let b = luma_thumbnail(&encode_bgra(&changed, w, h, stride, DEFAULT_QUALITY).unwrap()).unwrap();
        assert_eq!(a.len(), THUMB_W * THUMB_H);
        assert!(thumbnail_change(&a, &a2) < 0.001);
        let d = thumbnail_change(&a, &b);
        assert!(d > 0.02 && d < 0.2, "{d}");
        assert!(luma_thumbnail(b"not webp").is_none());
    }

    #[test]
    fn model_images_go_out_as_png() {
        let (w, h, stride) = (123u32, 45u32, 123 * 4);
        let webp = encode_bgra(&frame(w, h, stride), w, h, stride, DEFAULT_QUALITY).unwrap();
        let (png, mime) = for_model(webp.clone(), "image/webp", true);
        assert_eq!(for_model(webp.clone(), "image/webp", false), (webp, "image/webp".to_string()));
        assert_eq!(mime, "image/png");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!((u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap())), (w, h));
        // The pixel stream inflates to one filter byte plus an RGB row per line.
        let len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let mut raw = vec![];
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&png[41..41 + len]), &mut raw).unwrap();
        assert_eq!(raw.len(), (w as usize * 3 + 1) * h as usize);
        // Not WebP: passed through untouched.
        assert_eq!(for_model(b"\xff\xd8jpeg".to_vec(), "image/jpeg", true), (b"\xff\xd8jpeg".to_vec(), "image/jpeg".to_string()));
    }

    #[test]
    fn rejects_bad_geometry() {
        let p = frame(10, 10, 40);
        assert!(encode_bgra(&p, 10, 10, 39, 60.0).is_err());
        assert!(encode_bgra(&p, 10, 11, 40, 60.0).is_err());
        assert!(encode_bgra(&p, 0, 10, 40, 60.0).is_err());
        assert!(encode_bgra(&p, 10, 10, 40, 101.0).is_err());
        assert!(encode_bgra(&p[..399], 10, 10, 40, 60.0).is_err());
        // The last row needs only `width * 4` bytes, not a full stride.
        let q = frame(10, 10, 48);
        assert!(encode_bgra(&q[..48 * 9 + 40], 10, 10, 48, 60.0).is_ok());
    }
}
