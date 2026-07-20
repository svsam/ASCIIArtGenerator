//! Image-to-ASCII conversion and text export primitives.

mod convert;
mod export;
mod model;

pub use convert::convert;
#[cfg(feature = "desktop")]
pub use convert::decode_image;
#[cfg(feature = "desktop")]
pub use export::{BatchOutputPaths, ExportFormats, atomic_write, batch_output_paths};
pub use export::{render_ansi, render_plain};
#[cfg(feature = "desktop")]
pub use model::ImageLoadError;
pub use model::{
    AsciiCell, AsciiDocument, CharacterRamp, ConversionError, ConversionSettings, CropRect,
    DitherMode, RowSizing,
};
