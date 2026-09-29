//! Saving what a screen shows as a PNG.

use super::*;

/// Draws `elements` into an offscreen texture and saves it as a PNG, for `shot:` debug steps.
/// `scale` must be the output's: client surfaces size themselves by it when drawn.
pub fn save_png(
    renderer: &mut GlesRenderer,
    elements: &[OutputElement],
    size: Size<i32, Physical>,
    scale: f64,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let pixels = copy_frame(renderer, elements, size, scale)?;
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    encode_png(file, &pixels, size.w as u32, size.h as u32)?;
    Ok(())
}

/// Draws `elements` into an offscreen texture and reads it back as opaque RGBA bytes, a row at a
/// time from the top. `scale` must be the output's.
pub fn copy_frame<E: smithay::backend::renderer::element::RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    elements: &[E],
    size: Size<i32, Physical>,
    scale: f64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let buffer_size = Size::<i32, Buffer>::from((size.w, size.h));
    let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
    let mut target = renderer.bind(&mut texture)?;
    let mut damage = OutputDamageTracker::new(size, scale, Transform::Normal);
    damage.render_output(renderer, &mut target, 0, elements, BACKGROUND)?;
    let mapping =
        renderer.copy_framebuffer(&target, Rectangle::from_size(buffer_size), Fourcc::Abgr8888)?;
    let mut pixels = renderer.map_texture(&mapping)?.to_vec();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    Ok(pixels)
}

/// RGBA bytes, `w` × `h`, as a PNG written to `out`.
pub fn encode_png(
    out: impl std::io::Write,
    pixels: &[u8],
    w: u32,
    h: u32,
) -> Result<(), png::EncodingError> {
    let mut encoder = png::Encoder::new(out, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)
}
