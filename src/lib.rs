//! Image-to-ASCII conversion and text export primitives.

mod convert;
mod export;
mod model;

pub use convert::{convert, decode_image};
pub use export::{
    BatchOutputPaths, ExportFormats, atomic_write, batch_output_paths, render_ansi, render_plain,
};
pub use model::{
    AsciiCell, AsciiDocument, CharacterRamp, ConversionError, ConversionSettings, CropRect,
    DitherMode, ImageLoadError, RowSizing,
};
