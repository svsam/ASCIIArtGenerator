use std::{fs, io::Cursor, path::Path};

use ascii_art_generator::decode_image;
use image::{
    DynamicImage, Frame, ImageBuffer, ImageFormat, Rgb, Rgba, RgbaImage, codecs::gif::GifEncoder,
};

#[test]
fn decodes_every_advertised_static_format() {
    let directory = tempfile::tempdir().unwrap();
    let source = DynamicImage::ImageRgb8(ImageBuffer::from_fn(3, 2, |x, y| {
        Rgb([(x * 70 + 20) as u8, (y * 90 + 30) as u8, 140])
    }));
    let formats = [
        ("png", ImageFormat::Png),
        ("jpg", ImageFormat::Jpeg),
        ("webp", ImageFormat::WebP),
        ("bmp", ImageFormat::Bmp),
        ("tiff", ImageFormat::Tiff),
    ];

    for (extension, format) in formats {
        let path = directory.path().join(format!("fixture.{extension}"));
        write_dynamic_image(&source, format, &path);
        let decoded = decode_image(&path).unwrap_or_else(|error| {
            panic!("failed to decode {extension}: {error}");
        });
        assert_eq!(decoded.dimensions(), (3, 2), "wrong {extension} dimensions");
    }
}

#[test]
fn animated_gif_uses_its_first_frame() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("animated.gif");
    let first = RgbaImage::from_pixel(2, 1, Rgba([255, 0, 0, 255]));
    let second = RgbaImage::from_pixel(2, 1, Rgba([0, 255, 0, 255]));
    let mut bytes = Vec::new();
    GifEncoder::new(&mut bytes)
        .encode_frames([Frame::new(first), Frame::new(second)])
        .unwrap();
    fs::write(&path, bytes).unwrap();

    let decoded = decode_image(&path).unwrap();
    assert_eq!(decoded.dimensions(), (2, 1));
    assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
}

#[test]
fn png_alpha_channels_are_preserved_by_decoding() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("alpha.png");
    let source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([10, 20, 30, 64])));
    write_dynamic_image(&source, ImageFormat::Png, &path);

    assert_eq!(
        decode_image(&path).unwrap().get_pixel(0, 0).0,
        [10, 20, 30, 64]
    );
}

fn write_dynamic_image(image: &DynamicImage, format: ImageFormat, path: &Path) {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    fs::write(path, bytes.into_inner()).unwrap();
}
