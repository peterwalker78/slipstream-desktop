//! Turning a chosen picture into the PNG the desktop shows.
//!
//! The picture is opened with GTK's own image loading, so any format the system can open will
//! do (PNG, JPEG, WebP, GIF, TIFF, BMP, and AVIF, HEIC or JPEG XL where their loaders are
//! installed), and an animated one gives its first frame. It is shrunk if it is larger than any
//! screen needs and saved as a PNG at `slipstream_config::picture_path()`, which is the only
//! file Slipstream itself reads.

use std::path::Path;

use gtk::{gdk, glib, prelude::*};
use gtk4 as gtk;
use slipstream_config::{PICTURE_LONGEST, PICTURE_MOST_PIXELS};

/// Opens `source` and saves the prepared PNG at `target`, replacing what was there only once it
/// is complete. Returns the prepared size. A file that isn't a picture leaves `target` alone.
pub fn prepare(source: &Path, target: &Path) -> Result<(u32, u32), String> {
    let texture = gdk::Texture::from_filename(source).map_err(|err| err.to_string())?;
    let (w, h) = (texture.width() as u32, texture.height() as u32);
    let (tw, th) = bounded(w, h);
    let png = if (tw, th) == (w, h) {
        texture.save_to_png_bytes()
    } else {
        let mut downloader = gdk::TextureDownloader::new(&texture);
        downloader.set_format(gdk::MemoryFormat::R8g8b8a8Premultiplied);
        let (pixels, stride) = downloader.download_bytes();
        let smaller = shrink(&pixels, stride, (w, h), (tw, th));
        gdk::MemoryTexture::new(
            tw as i32,
            th as i32,
            gdk::MemoryFormat::R8g8b8a8Premultiplied,
            &glib::Bytes::from_owned(smaller),
            tw as usize * 4,
        )
        .save_to_png_bytes()
    };
    if let Some(folder) = target.parent() {
        std::fs::create_dir_all(folder).map_err(|err| err.to_string())?;
    }
    slipstream_config::replace(target, &png).map_err(|err| err.to_string())?;
    Ok((tw, th))
}

/// The size a `w` by `h` picture is kept at: as it is, or shrunk in proportion until its long
/// side and its pixel count are both within what the desktop takes.
pub fn bounded(w: u32, h: u32) -> (u32, u32) {
    let (w, h) = (w.max(1), h.max(1));
    let by_side = f64::from(PICTURE_LONGEST) / f64::from(w.max(h));
    let by_count = (PICTURE_MOST_PIXELS as f64 / (f64::from(w) * f64::from(h))).sqrt();
    let k = by_side.min(by_count);
    if k >= 1.0 {
        return (w, h);
    }
    let fit = |side: u32| ((f64::from(side) * k).floor() as u32).max(1);
    (fit(w), fit(h))
}

/// Shrinks RGBA `pixels` (rows `stride` bytes apart) from `from` to `to` by averaging each
/// block of source pixels that lands on one output pixel, which keeps fine detail from
/// shimmering the way skipping pixels would.
pub fn shrink(pixels: &[u8], stride: usize, from: (u32, u32), to: (u32, u32)) -> Vec<u8> {
    let (fw, fh) = (from.0 as usize, from.1 as usize);
    let (tw, th) = (to.0.max(1) as usize, to.1.max(1) as usize);
    let span = |at: usize, out: usize, of: usize| {
        let start = at * of / out;
        (start, ((at + 1) * of / out).max(start + 1).min(of))
    };
    let mut out = vec![0u8; tw * th * 4];
    let mut sums = vec![0u32; tw * 4];
    for oy in 0..th {
        let (y0, y1) = span(oy, th, fh);
        sums.fill(0);
        for y in y0..y1 {
            let row = &pixels[y * stride..y * stride + fw * 4];
            for ox in 0..tw {
                let (x0, x1) = span(ox, tw, fw);
                for pixel in row[x0 * 4..x1 * 4].as_chunks::<4>().0 {
                    for (sum, channel) in sums[ox * 4..ox * 4 + 4].iter_mut().zip(pixel) {
                        *sum += u32::from(*channel);
                    }
                }
            }
        }
        for ox in 0..tw {
            let (x0, x1) = span(ox, tw, fw);
            let count = ((x1 - x0) * (y1 - y0)) as u32;
            for c in 0..4 {
                out[(oy * tw + ox) * 4 + c] = ((sums[ox * 4 + c] + count / 2) / count) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("test-scratch")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_picture_is_kept_at_its_size_unless_it_is_larger_than_any_screen_needs() {
        assert_eq!(bounded(4000, 3000), (4000, 3000));
        assert_eq!(bounded(1, 1), (1, 1));
        // Long side first: a wide panorama keeps its shape at 7680 across.
        assert_eq!(bounded(15360, 2160), (7680, 1080));
        // Then the pixel count: a square that would be 59 megapixels at 7680.
        let (w, h) = bounded(10000, 10000);
        assert_eq!(w, h);
        assert!(u64::from(w) * u64::from(h) <= PICTURE_MOST_PIXELS);
        assert!(w < PICTURE_LONGEST && w > 5000);
        let (w, h) = bounded(8000, 12000);
        assert!(w.max(h) <= PICTURE_LONGEST);
        assert!(u64::from(w) * u64::from(h) <= PICTURE_MOST_PIXELS);
        assert!(
            ((w as f64 / h as f64) - 8.0 / 12.0).abs() < 0.01,
            "same shape"
        );
    }

    #[test]
    fn shrinking_averages_each_block() {
        // Four pixels across, two down, with rows padded to 20 bytes: black, white, red, blue.
        let mut pixels = vec![0u8; 40];
        for y in 0..2 {
            let row = &mut pixels[y * 20..y * 20 + 16];
            row[..4].copy_from_slice(&[0, 0, 0, 255]);
            row[4..8].copy_from_slice(&[255, 255, 255, 255]);
            row[8..12].copy_from_slice(&[255, 0, 0, 255]);
            row[12..16].copy_from_slice(&[0, 0, 255, 255]);
        }
        let out = shrink(&pixels, 20, (4, 2), (2, 1));
        assert_eq!(out, [128, 128, 128, 255, 128, 0, 128, 255]);
    }

    #[test]
    fn a_file_that_is_not_a_picture_is_refused_and_nothing_changes() {
        let dir = scratch("picture-file");
        let text = dir.join("notes.png");
        std::fs::write(&text, "these are words, not a picture\n").unwrap();
        let target = dir.join("wallpaper.png");
        std::fs::write(&target, b"the picture before").unwrap();
        assert!(prepare(&text, &target).is_err());
        assert!(prepare(&dir.join("missing.jpg"), &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"the picture before");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_picture_is_prepared_as_a_png_that_reads_back() {
        let dir = scratch("picture-made");
        let source = dir.join("small.png");
        let pixels = [10u8, 20, 30, 255].repeat(6);
        gdk::MemoryTexture::new(
            3,
            2,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(pixels),
            12,
        )
        .save_to_png(&source)
        .unwrap();
        let target = dir.join("out").join("wallpaper.png");
        assert_eq!(prepare(&source, &target), Ok((3, 2)));
        let back = gdk::Texture::from_filename(&target).unwrap();
        assert_eq!((back.width(), back.height()), (3, 2));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
