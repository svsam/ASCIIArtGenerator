//! Browser-facing WebAssembly adapter for the conversion engine.

use ascii_art_generator::{CharacterRamp, ConversionSettings, CropRect, convert, render_plain};
use image::RgbaImage;
use wasm_bindgen::prelude::*;

/// An RGBA source image retained in WebAssembly memory for repeated conversions.
#[wasm_bindgen]
pub struct AsciiImage {
    image: RgbaImage,
}

#[wasm_bindgen]
impl AsciiImage {
    /// Copies browser-decoded RGBA pixels into WebAssembly memory.
    #[wasm_bindgen(constructor)]
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<AsciiImage, JsError> {
        create_image(width, height, rgba)
            .map(|image| Self { image })
            .map_err(|message| JsError::new(&message))
    }

    /// Converts the retained image with the simple settings exposed by the web app.
    pub fn render(&self, columns: u32, ramp: &str) -> Result<String, JsError> {
        render_image(&self.image, columns, ramp).map_err(|message| JsError::new(&message))
    }
}

fn create_image(width: u32, height: u32, rgba: Vec<u8>) -> Result<RgbaImage, String> {
    if width == 0 || height == 0 {
        return Err("the source image dimensions must be greater than zero".to_owned());
    }

    let expected_length = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "the source image dimensions are too large".to_owned())?;
    if rgba.len() as u64 != expected_length {
        return Err(format!(
            "the RGBA buffer has {} bytes but {expected_length} were expected",
            rgba.len()
        ));
    }

    RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "the RGBA buffer could not be represented as an image".to_owned())
}

fn render_image(image: &RgbaImage, columns: u32, ramp: &str) -> Result<String, String> {
    let settings = ConversionSettings {
        columns,
        ramp: CharacterRamp::new("Web", ramp),
        ..ConversionSettings::default()
    };
    convert(image, CropRect::FULL, &settings)
        .map(|document| render_plain(&document))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use ascii_art_generator::{RowSizing, convert, render_plain};
    use image::{Rgba, RgbaImage};

    use super::*;

    #[test]
    fn validates_dimensions_and_rgba_length() {
        assert!(create_image(0, 1, Vec::new()).is_err());
        assert!(create_image(1, 1, vec![0, 0, 0]).is_err());
        assert!(create_image(1, 1, vec![0, 0, 0, 255]).is_ok());
    }

    #[test]
    fn rejects_invalid_columns_and_ramps() {
        let image = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 255]));
        assert!(render_image(&image, 0, "@ ").is_err());
        assert!(render_image(&image, 1, "@").is_err());
        assert!(render_image(&image, 1, "@▓ ").is_err());
    }

    #[test]
    fn renders_black_and_white_with_lf_line_endings() {
        let mut image = RgbaImage::new(2, 1);
        image.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        image.put_pixel(1, 0, Rgba([255, 255, 255, 255]));
        assert_eq!(render_image(&image, 2, "@ ").unwrap(), "@ \n");
    }

    #[test]
    fn transparent_pixels_use_the_default_white_matte() {
        let image = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 0]));
        assert_eq!(render_image(&image, 1, "@ ").unwrap(), " \n");
    }

    #[test]
    fn adapter_matches_direct_core_conversion() {
        let image = RgbaImage::from_fn(8, 4, |x, y| {
            Rgba([(x * 31) as u8, (y * 63) as u8, 127, 255])
        });
        let settings = ConversionSettings {
            columns: 6,
            row_sizing: RowSizing::Auto {
                character_cell_ratio: 0.5,
            },
            ramp: CharacterRamp::new("Web", "@#:. "),
            ..ConversionSettings::default()
        };
        let expected = render_plain(&convert(&image, CropRect::FULL, &settings).unwrap());
        assert_eq!(render_image(&image, 6, "@#:. ").unwrap(), expected);
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use wasm_bindgen_test::*;

    use super::*;

    #[wasm_bindgen_test]
    fn exported_class_retains_and_renders_an_image() {
        let image = AsciiImage::new(1, 1, vec![0, 0, 0, 255]).unwrap();
        assert_eq!(image.render(1, "@ ").unwrap(), "@\n");
    }
}
