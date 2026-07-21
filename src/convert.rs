#[cfg(feature = "desktop")]
use std::path::Path;

use image::RgbaImage;
#[cfg(feature = "desktop")]
use image::{DynamicImage, ImageDecoder, ImageReader};

#[cfg(feature = "desktop")]
use crate::model::ImageLoadError;
use crate::model::{
    AsciiCell, AsciiDocument, ConversionError, ConversionSettings, CropRect, DitherMode,
    MAX_OUTPUT_CELLS, RowSizing,
};

#[cfg(feature = "desktop")]
pub fn decode_image(path: impl AsRef<Path>) -> Result<RgbaImage, ImageLoadError> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image.into_rgba8())
}

pub fn convert(
    image: &RgbaImage,
    crop: CropRect,
    settings: &ConversionSettings,
) -> Result<AsciiDocument, ConversionError> {
    settings.validate()?;
    crop.validate()?;
    if image.width() == 0 || image.height() == 0 {
        return Err(ConversionError::EmptyImage);
    }

    let width = settings.columns;
    let crop_aspect = image.height() as f32 * crop.height / (image.width() as f32 * crop.width);
    let height = match settings.row_sizing {
        RowSizing::Auto {
            character_cell_ratio,
        } => (crop_aspect * width as f32 * character_cell_ratio)
            .round()
            .max(1.0) as u32,
        RowSizing::Exact(rows) => rows,
    };
    if !(1..=1_000).contains(&height) {
        return Err(ConversionError::InvalidRows);
    }
    let cell_count = width as usize * height as usize;
    if cell_count > MAX_OUTPUT_CELLS {
        return Err(ConversionError::OutputTooLarge);
    }

    let matte = settings.transparency_matte.map(srgb_u8_to_linear);
    let mut colors = Vec::with_capacity(cell_count);
    let mut luminances = Vec::with_capacity(cell_count);
    let source_width = image.width() as f32;
    let source_height = image.height() as f32;
    let crop_x0 = crop.x * source_width;
    let crop_y0 = crop.y * source_height;
    let crop_width = crop.width * source_width;
    let crop_height = crop.height * source_height;

    for row in 0..height {
        let y0 = crop_y0 + crop_height * row as f32 / height as f32;
        let y1 = crop_y0 + crop_height * (row + 1) as f32 / height as f32;
        for column in 0..width {
            let x0 = crop_x0 + crop_width * column as f32 / width as f32;
            let x1 = crop_x0 + crop_width * (column + 1) as f32 / width as f32;
            let averaged = average_linear_pixel(image, x0, y0, x1, y1, matte);
            let adjusted = adjust_color(averaged, settings);
            luminances.push(luminance(adjusted));
            colors.push(adjusted.map(linear_to_srgb_u8));
        }
    }

    let ramp = settings.ramp.validate()?;
    let indices = dither_indices(&mut luminances, width, height, ramp.len(), settings.dither);
    let cells = indices
        .into_iter()
        .zip(colors)
        .map(|(index, rgb)| {
            let index = if settings.invert_density {
                index
            } else {
                ramp.len() - 1 - index
            };
            AsciiCell {
                character: ramp[index],
                rgb,
            }
        })
        .collect();

    Ok(AsciiDocument {
        width,
        height,
        cells,
    })
}

fn average_linear_pixel(
    image: &RgbaImage,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    matte: [f32; 3],
) -> [f32; 3] {
    let mut total = [0.0; 3];
    let mut total_weight = 0.0;
    let max_x = image.width() as i32 - 1;
    let max_y = image.height() as i32 - 1;

    for source_y in y0.floor() as i32..y1.ceil() as i32 {
        let clamped_y = source_y.clamp(0, max_y) as u32;
        let y_weight = (y1.min(source_y as f32 + 1.0) - y0.max(source_y as f32)).max(0.0);
        for source_x in x0.floor() as i32..x1.ceil() as i32 {
            let clamped_x = source_x.clamp(0, max_x) as u32;
            let x_weight = (x1.min(source_x as f32 + 1.0) - x0.max(source_x as f32)).max(0.0);
            let weight = x_weight * y_weight;
            if weight == 0.0 {
                continue;
            }
            let pixel = image.get_pixel(clamped_x, clamped_y).0;
            let alpha = pixel[3] as f32 / 255.0;
            for channel in 0..3 {
                let source = srgb_u8_to_linear(pixel[channel]);
                total[channel] += (source * alpha + matte[channel] * (1.0 - alpha)) * weight;
            }
            total_weight += weight;
        }
    }

    if total_weight > 0.0 {
        total.map(|channel| channel / total_weight)
    } else {
        matte
    }
}

fn adjust_color(mut color: [f32; 3], settings: &ConversionSettings) -> [f32; 3] {
    for (channel, gain) in color.iter_mut().zip(settings.rgb_gain) {
        *channel *= gain;
    }
    let gray = luminance(color);
    for channel in &mut color {
        *channel = gray + (*channel - gray) * settings.saturation;
        *channel = ((*channel - 0.5) * settings.contrast + 0.5 + settings.brightness)
            .clamp(0.0, 1.0)
            .powf(1.0 / settings.gamma);
    }
    color
}

fn dither_indices(
    luminances: &mut [f32],
    width: u32,
    height: u32,
    levels: usize,
    mode: DitherMode,
) -> Vec<usize> {
    let max_index = levels - 1;
    let quantize =
        |value: f32| -> usize { (value.clamp(0.0, 1.0) * max_index as f32).round() as usize };

    match mode {
        DitherMode::None => luminances.iter().map(|value| quantize(*value)).collect(),
        DitherMode::Bayer4x4 => {
            const BAYER: [[f32; 4]; 4] = [
                [0.0, 8.0, 2.0, 10.0],
                [12.0, 4.0, 14.0, 6.0],
                [3.0, 11.0, 1.0, 9.0],
                [15.0, 7.0, 13.0, 5.0],
            ];
            let amplitude = 1.0 / max_index.max(1) as f32;
            luminances
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let x = index % width as usize;
                    let y = index / width as usize;
                    let threshold = (BAYER[y % 4][x % 4] + 0.5) / 16.0 - 0.5;
                    quantize(*value + threshold * amplitude)
                })
                .collect()
        }
        DitherMode::FloydSteinberg => {
            let mut result = vec![0; luminances.len()];
            let width = width as usize;
            let height = height as usize;
            for y in 0..height {
                for x in 0..width {
                    let index = y * width + x;
                    let old = luminances[index].clamp(0.0, 1.0);
                    let ramp_index = quantize(old);
                    result[index] = ramp_index;
                    let new = ramp_index as f32 / max_index.max(1) as f32;
                    let error = old - new;
                    diffuse(luminances, width, height, x + 1, y, error * 7.0 / 16.0);
                    if x > 0 {
                        diffuse(luminances, width, height, x - 1, y + 1, error * 3.0 / 16.0);
                    }
                    diffuse(luminances, width, height, x, y + 1, error * 5.0 / 16.0);
                    diffuse(luminances, width, height, x + 1, y + 1, error / 16.0);
                }
            }
            result
        }
    }
}

fn diffuse(values: &mut [f32], width: usize, height: usize, x: usize, y: usize, error: f32) {
    if x < width && y < height {
        values[y * width + x] += error;
    }
}

fn luminance(color: [f32; 3]) -> f32 {
    0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
}

fn srgb_u8_to_linear(channel: u8) -> f32 {
    let channel = channel as f32 / 255.0;
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_u8(channel: f32) -> u8 {
    let channel = channel.clamp(0.0, 1.0);
    let encoded = if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;

    fn exact_settings(columns: u32, rows: u32, ramp: &str) -> ConversionSettings {
        ConversionSettings {
            columns,
            row_sizing: RowSizing::Exact(rows),
            ramp: crate::CharacterRamp::new("test", ramp),
            ..ConversionSettings::default()
        }
    }

    #[test]
    fn maps_black_and_white_to_ramp_extremes() {
        let mut image = RgbaImage::new(2, 1);
        image.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        image.put_pixel(1, 0, Rgba([255, 255, 255, 255]));
        let document = convert(&image, CropRect::FULL, &exact_settings(2, 1, ".|")).unwrap();
        assert_eq!(document.cells[0].character, '|');
        assert_eq!(document.cells[1].character, '.');
    }

    #[test]
    fn invert_changes_density_but_not_color() {
        let image = RgbaImage::from_pixel(1, 1, Rgba([25, 50, 75, 255]));
        let regular = convert(&image, CropRect::FULL, &exact_settings(1, 1, ".|")).unwrap();
        assert_eq!(regular.cells[0].rgb, [25, 50, 75]);
        let mut inverted_settings = exact_settings(1, 1, ".|");
        inverted_settings.invert_density = true;
        let inverted = convert(&image, CropRect::FULL, &inverted_settings).unwrap();
        assert_ne!(regular.cells[0].character, inverted.cells[0].character);
        assert_eq!(regular.cells[0].rgb, inverted.cells[0].rgb);
    }

    #[test]
    fn transparent_pixels_use_the_selected_matte() {
        let image = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 0]));
        let mut settings = exact_settings(1, 1, ".|");
        settings.transparency_matte = [255, 0, 0];
        let document = convert(&image, CropRect::FULL, &settings).unwrap();
        assert_eq!(document.cells[0].rgb, [255, 0, 0]);
    }

    #[test]
    fn auto_height_uses_crop_and_character_aspects() {
        let image = RgbaImage::new(400, 200);
        let settings = ConversionSettings {
            columns: 100,
            row_sizing: RowSizing::Auto {
                character_cell_ratio: 0.5,
            },
            ..ConversionSettings::default()
        };
        let document = convert(&image, CropRect::FULL, &settings).unwrap();
        assert_eq!((document.width, document.height), (100, 25));
    }

    #[test]
    fn dithering_modes_are_deterministic_and_distinct() {
        let image = RgbaImage::from_fn(8, 4, |x, _| {
            let value = (x * 255 / 7) as u8;
            Rgba([value, value, value, 255])
        });
        let mut settings = exact_settings(8, 4, " .:*#@");
        let plain = convert(&image, CropRect::FULL, &settings).unwrap();
        settings.dither = DitherMode::FloydSteinberg;
        let floyd = convert(&image, CropRect::FULL, &settings).unwrap();
        settings.dither = DitherMode::Bayer4x4;
        let bayer = convert(&image, CropRect::FULL, &settings).unwrap();
        assert_ne!(plain.cells, floyd.cells);
        assert_ne!(plain.cells, bayer.cells);
    }
}
