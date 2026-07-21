use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_OUTPUT_CELLS: usize = 1_000_000;

/// A normalized crop rectangle. Coordinates are measured from the oriented source image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CropRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl CropRect {
    pub const FULL: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    pub fn validate(self) -> Result<(), ConversionError> {
        let values = [self.x, self.y, self.width, self.height];
        if values.iter().any(|value| !value.is_finite())
            || self.x < 0.0
            || self.y < 0.0
            || self.width <= 0.0
            || self.height <= 0.0
            || self.x + self.width > 1.000_001
            || self.y + self.height > 1.000_001
        {
            return Err(ConversionError::InvalidCrop);
        }
        Ok(())
    }
}

impl Default for CropRect {
    fn default() -> Self {
        Self::FULL
    }
}

/// Controls whether output rows follow the image aspect ratio or are explicitly chosen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RowSizing {
    Auto { character_cell_ratio: f32 },
    Exact(u32),
}

impl Default for RowSizing {
    fn default() -> Self {
        Self::Auto {
            character_cell_ratio: 0.5,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DitherMode {
    #[default]
    None,
    FloydSteinberg,
    Bayer4x4,
}

/// A density ramp ordered from the lightest glyph to the darkest glyph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterRamp {
    pub name: String,
    pub characters: String,
}

impl CharacterRamp {
    pub fn new(name: impl Into<String>, characters: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            characters: characters.into(),
        }
    }

    pub fn validate(&self) -> Result<Vec<char>, ConversionError> {
        let characters: Vec<char> = self.characters.chars().collect();
        if !(2..=256).contains(&characters.len()) {
            return Err(ConversionError::InvalidRampLength);
        }
        if characters
            .iter()
            .any(|character| !matches!(character, ' '..='~'))
        {
            return Err(ConversionError::InvalidRampCharacter);
        }
        Ok(characters)
    }

    pub fn built_ins() -> Vec<Self> {
        vec![
            Self::new("Classic", ".:-=+*#%@|"),
            Self::new("Compact", " .:*#@"),
            Self::new("Detailed", " .,:;-=+*#%@$NWM"),
        ]
    }
}

impl Default for CharacterRamp {
    fn default() -> Self {
        Self::built_ins().remove(0)
    }
}

/// All settings which affect the generated document. Adjustment values operate in linear sRGB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversionSettings {
    pub columns: u32,
    pub row_sizing: RowSizing,
    pub ramp: CharacterRamp,
    pub dither: DitherMode,
    pub invert_density: bool,
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub saturation: f32,
    pub rgb_gain: [f32; 3],
    pub transparency_matte: [u8; 3],
}

impl ConversionSettings {
    pub fn validate(&self) -> Result<(), ConversionError> {
        if !(1..=1_000).contains(&self.columns) {
            return Err(ConversionError::InvalidColumns);
        }
        if let RowSizing::Auto {
            character_cell_ratio,
        } = self.row_sizing
            && (!character_cell_ratio.is_finite() || !(0.1..=2.0).contains(&character_cell_ratio))
        {
            return Err(ConversionError::InvalidCellRatio);
        }
        if let RowSizing::Exact(rows) = self.row_sizing
            && !(1..=1_000).contains(&rows)
        {
            return Err(ConversionError::InvalidRows);
        }
        if !(-1.0..=1.0).contains(&self.brightness)
            || !(0.0..=3.0).contains(&self.contrast)
            || !(0.2..=3.0).contains(&self.gamma)
            || !(0.0..=3.0).contains(&self.saturation)
            || self
                .rgb_gain
                .iter()
                .any(|gain| !gain.is_finite() || !(0.0..=3.0).contains(gain))
        {
            return Err(ConversionError::InvalidAdjustment);
        }
        self.ramp.validate()?;
        Ok(())
    }
}

impl Default for ConversionSettings {
    fn default() -> Self {
        Self {
            columns: 120,
            row_sizing: RowSizing::default(),
            ramp: CharacterRamp::default(),
            dither: DitherMode::None,
            invert_density: false,
            brightness: 0.0,
            contrast: 1.0,
            gamma: 1.0,
            saturation: 1.0,
            rgb_gain: [1.0; 3],
            transparency_matte: [255; 3],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsciiCell {
    pub character: char,
    pub rgb: [u8; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsciiDocument {
    pub width: u32,
    pub height: u32,
    pub cells: Vec<AsciiCell>,
}

impl AsciiDocument {
    pub fn row(&self, row: u32) -> Option<&[AsciiCell]> {
        if row >= self.height {
            return None;
        }
        let start = row as usize * self.width as usize;
        Some(&self.cells[start..start + self.width as usize])
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ConversionError {
    #[error("the source image has no pixels")]
    EmptyImage,
    #[error("the crop rectangle must be non-empty and remain inside the source image")]
    InvalidCrop,
    #[error("the output width must be between 1 and 1000 columns")]
    InvalidColumns,
    #[error("the output height must be between 1 and 1000 rows")]
    InvalidRows,
    #[error("the character cell ratio must be between 0.1 and 2.0")]
    InvalidCellRatio,
    #[error("brightness, contrast, gamma, saturation, or RGB gain is outside its valid range")]
    InvalidAdjustment,
    #[error("a character ramp must contain between 2 and 256 characters")]
    InvalidRampLength,
    #[error("character ramps may contain printable ASCII characters only")]
    InvalidRampCharacter,
    #[error("the requested output exceeds one million characters")]
    OutputTooLarge,
}

#[cfg(feature = "desktop")]
#[derive(Debug, Error)]
pub enum ImageLoadError {
    #[error("could not read the image: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not decode the image: {0}")]
    Decode(#[from] image::ImageError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_validation_rejects_out_of_bounds_rectangles() {
        assert!(CropRect::FULL.validate().is_ok());
        assert!(
            CropRect {
                x: 0.8,
                width: 0.3,
                ..CropRect::FULL
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn ramps_are_strict_ascii_but_allow_duplicates() {
        assert!(CharacterRamp::new("valid", "@@. ").validate().is_ok());
        assert!(CharacterRamp::new("unicode", "@▓ ").validate().is_err());
        assert!(CharacterRamp::new("newline", "@\n ").validate().is_err());
    }

    #[test]
    fn classic_ramp_runs_from_light_to_dark() {
        assert_eq!(CharacterRamp::default().characters, ".:-=+*#%@|");
    }

    #[test]
    fn settings_round_trip_through_serde() {
        let settings = ConversionSettings::default();
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<ConversionSettings>(&json).unwrap(),
            settings
        );
    }
}
