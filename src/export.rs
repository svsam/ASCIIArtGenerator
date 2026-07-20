use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use tempfile::Builder;

use crate::AsciiDocument;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportFormats {
    pub plain: bool,
    pub ansi: bool,
}

impl Default for ExportFormats {
    fn default() -> Self {
        Self {
            plain: true,
            ansi: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchOutputPaths {
    pub plain: Option<PathBuf>,
    pub ansi: Option<PathBuf>,
}

pub fn render_plain(document: &AsciiDocument) -> String {
    let mut output = String::with_capacity(document.cells.len() + document.height as usize);
    for row in 0..document.height {
        if let Some(cells) = document.row(row) {
            output.extend(cells.iter().map(|cell| cell.character));
        }
        output.push('\n');
    }
    output
}

pub fn render_ansi(document: &AsciiDocument) -> String {
    let mut output = String::with_capacity(document.cells.len() * 8);
    for row in 0..document.height {
        let mut active_color = None;
        if let Some(cells) = document.row(row) {
            for cell in cells {
                if cell.character != ' ' && active_color != Some(cell.rgb) {
                    use std::fmt::Write as _;
                    let _ = write!(
                        output,
                        "\u{1b}[38;2;{};{};{}m",
                        cell.rgb[0], cell.rgb[1], cell.rgb[2]
                    );
                    active_color = Some(cell.rgb);
                }
                output.push(cell.character);
            }
        }
        if active_color.is_some() {
            output.push_str("\u{1b}[0m");
        }
        output.push('\n');
    }
    output
}

/// Produces deterministic, non-colliding names for a batch in queue order.
pub fn batch_output_paths(
    sources: &[PathBuf],
    destination: &Path,
    formats: ExportFormats,
) -> Vec<BatchOutputPaths> {
    let mut reserved = HashSet::new();
    sources
        .iter()
        .map(|source| {
            let stem = source
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|stem| !stem.is_empty())
                .unwrap_or("image");
            let mut suffix = 1;
            let base = loop {
                let candidate = if suffix == 1 {
                    format!("{stem}_ascii")
                } else {
                    format!("{stem}_ascii_{suffix}")
                };
                let plain = destination.join(format!("{candidate}.txt"));
                let ansi = destination.join(format!("{candidate}.ansi.txt"));
                let available = (!formats.plain || !reserved.contains(&plain))
                    && (!formats.ansi || !reserved.contains(&ansi));
                if available {
                    if formats.plain {
                        reserved.insert(plain);
                    }
                    if formats.ansi {
                        reserved.insert(ansi);
                    }
                    break candidate;
                }
                suffix += 1;
            };
            BatchOutputPaths {
                plain: formats
                    .plain
                    .then(|| destination.join(format!("{base}.txt"))),
                ansi: formats
                    .ansi
                    .then(|| destination.join(format!("{base}.ansi.txt"))),
            }
        })
        .collect()
}

/// Writes beside the destination and then atomically persists the completed file.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = Builder::new().prefix(".ascii-art-").tempfile_in(parent)?;
    temporary.write_all(contents)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AsciiCell;

    fn sample() -> AsciiDocument {
        AsciiDocument {
            width: 3,
            height: 1,
            cells: vec![
                AsciiCell {
                    character: '@',
                    rgb: [1, 2, 3],
                },
                AsciiCell {
                    character: '#',
                    rgb: [1, 2, 3],
                },
                AsciiCell {
                    character: '.',
                    rgb: [4, 5, 6],
                },
            ],
        }
    }

    #[test]
    fn plain_output_has_lf_and_no_escape_codes() {
        let output = render_plain(&sample());
        assert_eq!(output, "@#.\n");
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn ansi_output_coalesces_colors_and_resets_each_line() {
        assert_eq!(
            render_ansi(&sample()),
            "\u{1b}[38;2;1;2;3m@#\u{1b}[38;2;4;5;6m.\u{1b}[0m\n"
        );
    }

    #[test]
    fn duplicate_stems_receive_stable_suffixes() {
        let paths = batch_output_paths(
            &[PathBuf::from("a/photo.png"), PathBuf::from("b/photo.jpg")],
            Path::new("out"),
            ExportFormats {
                plain: true,
                ansi: true,
            },
        );
        assert_eq!(paths[0].plain, Some(PathBuf::from("out/photo_ascii.txt")));
        assert_eq!(
            paths[1].ansi,
            Some(PathBuf::from("out/photo_ascii_2.ansi.txt"))
        );
    }
}
