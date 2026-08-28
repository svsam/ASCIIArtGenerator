# ASCII Art Generator

ASCII Art Generator is a cross-platform Rust desktop editor that turns images into text you can save, share, paste into a terminal, or use wherever a picture is not an option. It exports portable monochrome `.txt` files and, when you want to keep the image's colour, 24-bit ANSI-colour `.ansi.txt` files.

## The problem

Images do not always belong in an image file. Sometimes the nicest version of a
picture is a terminal banner, text art in a README, or something that can travel
in a plain-text file. Quick online converters often hide the choices that matter:
how cells are sampled, which characters represent brightness, what happens to
transparency, and whether colour survives the export.

## The approach

The project keeps one platform-independent Rust conversion core and puts two
interfaces around it. The desktop editor adds cropping, queues, dithering,
presets, background work, and plain or ANSI-colour export. The browser edition
compiles the same core to WebAssembly, uses a Web Worker to keep conversion off
the main thread, and processes the selected image locally.

Pixels are area-averaged in linear sRGB, blended against a chosen transparency
matte, adjusted for tone and colour, and mapped to a light-to-dark character
ramp using Rec.709 luminance. The converter stores both the selected character
and its adjusted colour so one result can drive monochrome text, a coloured
preview, or 24-bit ANSI output.

## What I found

The conversion itself was only half of the problem. A useful editor also has to
protect the interface from slow or stale work. On desktop, debounced jobs carry
generation numbers so an old conversion cannot replace a newer edit. In the
browser, one worker owns the current Wasm image and coalesces rapid setting
changes. That structure made it possible to share the important algorithm while
letting each platform use its native file, clipboard, and interface APIs.

The current result is a working native editor and a deliberately smaller static
web app. The web version does not yet include desktop cropping, queues, dithering,
ANSI export, presets, or batch processing; those omissions are current scope, not
features hidden behind the interface.

## A quick example

The supplied example below shows the workflow in one window: the image and crop are at the upper left, conversion controls are on the right, and the generated ASCII preview is underneath.

![ASCII Art Generator converting an image into a monochrome ASCII preview](Example.png)

In this example, the app reads the bright figure, the dark scene around it, and the warm orange light. It reduces that information into a grid of characters while keeping enough contrast for the subject to remain recognisable. Switching on ANSI colour keeps the sampled colours too, for terminals that support true colour.

## Run the Rust desktop application

### From source

This project uses Rust 1.92.0 or newer and includes a `rust-toolchain.toml` file to select the right toolchain automatically.

1. Install Rust with [rustup](https://rustup.rs/).
2. Open a terminal in this project folder.
3. Run:

```shell
cargo run --release
```

The first build downloads dependencies and can take a few minutes. After that, `cargo run --release` launches the editor. You can also run the built executable directly from `target/release/` (`ascii-art-generator.exe` on Windows).

### Platform notes

- **Windows:** building from source may require the Visual Studio C++ Build Tools with the Desktop development with C++ workload.
- **macOS:** install the Xcode Command Line Tools if Rust asks for a linker: `xcode-select --install`.
- **Linux:** install the native build and windowing dependencies. For Ubuntu or Debian:

```shell
sudo apt-get install build-essential pkg-config libdbus-1-dev libxkbcommon-dev libwayland-dev libx11-dev libxi-dev libgl1-mesa-dev
```

At runtime, Linux also needs an XDG Desktop Portal backend such as `xdg-desktop-portal-gtk` or `xdg-desktop-portal-kde`. `zenity` is used as a file-dialog fallback.

## Run the web app

The repository also contains a standalone browser editor styled after the desktop application. It accepts one image at a time, lets you change the light-to-dark character ramp, output width, tone, RGB gains, and transparency matte, and produces monochrome ASCII that you can copy or download. Conversion runs locally in a Web Worker through WebAssembly; the source image is never uploaded.

The web build needs [Node.js](https://nodejs.org/) 18 or newer, the Rust WebAssembly target, and [`wasm-pack`](https://wasm-bindgen.github.io/wasm-pack/):

```shell
rustup target add wasm32-unknown-unknown
cargo install wasm-pack --version 0.15.0 --locked
npm ci
npm run build:web
npm run serve:web
```

Open `http://127.0.0.1:4173` after the server starts. The complete static site is generated in `dist/web/`; copy that directory to any static web host. It must be served over HTTP rather than opened directly from disk because the JavaScript loads the Wasm module as an ES module. Production hosting should use HTTPS so browsers permit the **Copy** button to access the clipboard.

Image decoding uses the browser, with PNG, JPEG, WebP, and the first frame of GIF as the primary supported formats. Other image types work when the visitor's browser can decode them. Very large sources are sampled down to a maximum edge of 4096 pixels before conversion.

## Rust core and desktop application

### Workspace and feature design

The project was built as a Cargo workspace so the conversion engine could be written once and used by two different interfaces. The root `ascii-art-generator` package contains both a reusable Rust library and the native desktop binary. The `web-wasm` workspace member is a small adapter that compiles the library for the browser.

The root crate enables its `desktop` feature by default. That feature adds `eframe`/`egui` for the interface, `rfd` for native file dialogs, `tempfile` for atomic exports, and the image decoders needed by the desktop program. The browser adapter depends on the root crate with `default-features = false`, so GUI code, filesystem access, dialogs, native decoding, and temporary-file support are not pulled into the Wasm build. The platform-independent model, conversion, and string-rendering code remain available in both targets.

| Rust file | Responsibility |
| --- | --- |
| `Cargo.toml` | Defines the workspace, default desktop feature, optional native dependencies, binary target, and release profile. |
| `src/model.rs` | Holds validated crops, row sizing, character ramps, tone settings, dithering modes, output cells, documents, and readable error types. |
| `src/convert.rs` | Decodes native images when the desktop feature is active and performs the shared pixel-to-character conversion. |
| `src/export.rs` | Renders documents as LF-terminated plain text or 24-bit ANSI text and, on desktop, creates safe batch names and atomic files. |
| `src/lib.rs` | Exposes the stable conversion and rendering API without exposing internal module layout. |
| `src/app.rs` | Implements desktop state, queue management, crop editing, settings, background jobs, previews, persistence, and export orchestration. |
| `src/main.rs` | Creates the native window and starts the `eframe` application. |

The shared model is deliberately stricter than either user interface. `ConversionSettings::validate` enforces dimensions, adjustment ranges, and printable ASCII ramps, while `convert` applies the one-million-cell safety ceiling after calculating the output height. `CropRect` uses normalized coordinates from `0.0` to `1.0`, so crops remain independent of the display size and source resolution. Each `AsciiCell` stores both its selected character and adjusted RGB colour; this lets the same converted `AsciiDocument` drive monochrome text, coloured desktop previews, and ANSI export.

### Conversion algorithm

The program does not try to identify objects in the image. It works directly with the pixels, which keeps the result predictable and gives you control over the artistic choices.

1. It loads the image, applies supported EXIF rotation, and reads the red, green, blue, and transparency value for every pixel.
2. It uses your crop and divides that area into character-sized cells. Pixels covered by each cell are area-averaged, so a small output still represents the source cleanly.
3. Transparent pixels are blended against the matte colour you choose. RGB gains, saturation, brightness, contrast, and gamma are then applied in a consistent order.
4. It measures each cell's perceived brightness using the Rec.709 luminance formula. Dark cells select dense characters and light cells select sparse characters from the chosen ramp.
5. If ANSI colour is enabled, the character keeps the sampled RGB colour while its density still comes from brightness. The result is ordinary text plus terminal colour escape sequences.

The standard ramp is `.:-=+*#%@|`, ordered from light/sparse to dark/dense. A shadow will usually use a character near the dense end, while a highlight will use a dot or another sparse character. **Invert density** swaps that relationship without changing the detected colour.

Internally, sampling is performed in linear sRGB rather than directly on gamma-compressed byte values. For every output cell, the converter calculates the exact source rectangle covered by that cell and area-weights every partially or fully covered pixel. Alpha is applied against the selected matte during this averaging step. This avoids the jagged or biased result produced by choosing only one source pixel per character.

Tone processing then follows a fixed order: RGB gain, saturation around the Rec.709 grey value, contrast and brightness, clamping, and gamma. The adjusted Rec.709 luminance is quantized into an index. Floyd–Steinberg can diffuse quantization error to neighbouring cells, while Bayer mode applies a repeatable 4×4 threshold matrix. The index is mapped onto the light-to-dark ramp, and the adjusted RGB value is retained alongside the character.

Automatic row sizing uses the cropped image aspect ratio, requested column count, and a default `0.5` character-cell ratio. This compensates for monospace characters being taller than they are wide. Exact row sizing is also available on desktop. Every renderer appends `\n` explicitly, which makes output deterministic and ensures exported files use LF line endings on every operating system.

### Desktop editor architecture

The native interface uses `eframe` and immediate-mode `egui`. `AsciiArtApp` owns a queue of image records and a smaller persisted settings object. Each image record contains its path, decoded pixels, display texture, normalized crop, optional settings override, latest ASCII document, loading state, conversion generation, and any error. The source/crop pane, ASCII preview, queue, and settings regions are resizable; `egui` memory preserves the panel layout.

Loading, conversion, and export do not run on the UI thread. The application creates two named worker threads connected through standard Rust channels. A load job decodes EXIF orientation, retains the full `RgbaImage` in an `Arc`, and makes a thumbnail of at most 2048×2048 pixels for the GPU preview. A conversion job clones the cheap `Arc`, crop, and settings values and sends the finished document back to the interface.

Interactive changes are debounced for 150 milliseconds. Every queue item has a generation number that is incremented when its crop or effective settings change. Results carrying an older generation are ignored, so a slow conversion cannot overwrite a newer edit. Shared settings mark only images without overrides as dirty; per-image overrides isolate changes to one queue item.

Desktop preferences are serialized by `eframe`. They include shared conversion settings, presets, theme, preview choices, export formats, and the last export directory, but deliberately exclude source images and queue contents. The persistence key is versioned, and the current migration reverses ramps saved under the former dark-to-light convention so existing artwork keeps the same density behavior.

Export jobs perform a fresh conversion from the original pixels rather than relying on a potentially scaled screen preview. Plain rendering writes one character per cell. ANSI rendering changes the terminal foreground colour only when needed, skips colour sequences for spaces, resets at line boundaries, and finishes with ordinary LF line endings. Files are written to a temporary sibling, synchronized, and then persisted over the destination so a failed or cancelled export does not leave a partially written result.

## Rust desktop editor guide

Start by choosing **Open images** or by dropping image files onto the window. The image queue supports PNG, JPEG, GIF, WebP, BMP, and TIFF. EXIF orientation is respected, and animated GIF/WebP files use their first frame.

| Feature | What it does |
| --- | --- |
| **Image queue** | Holds one or more source images. Select an item to edit it, reorder or remove it, and export one image or the whole queue. |
| **Source and crop** | Drag the crop outline or its corner handles to choose the part of the image that becomes ASCII art. Crops always belong to that individual image. Use **Reset crop** to restore the full source. Drag the divider below this pane to give the source or ASCII preview more space. |
| **Output size** | Choose a column count and the editor calculates a matching row count using the character-cell ratio. Turn on **Exact height** when you need a specific number of rows. A safety limit prevents accidentally creating more than one million cells. |
| **Character ramp** | Pick Classic, Compact, or Detailed, or enter a custom light-to-dark ramp. Classic uses `.:-=+*#%@|`. Custom ramps accept 2 to 256 printable ASCII characters, so the exported plain text remains portable. |
| **Tone and colour** | Tune red, green, and blue gains, saturation, brightness, contrast, gamma, and the transparency matte. These controls change how the image is interpreted before characters are chosen. |
| **Dithering** | Choose none, Floyd-Steinberg, or 4x4 Bayer. Dithering adds controlled variation to character density, which can preserve the feeling of gradients without changing sampled ANSI colours. |
| **Preview** | Switch between monochrome and ANSI-colour previews, adjust zoom, copy the shown art, and use the dark/light editor theme that is easiest on your eyes. |
| **Shared settings and overrides** | Most settings are shared by the queue. Create a per-image override when one image needs different treatment, reset it to the shared values, or apply the current settings to every item. |
| **Presets** | Save a named group of conversion settings and apply it later. Presets do not contain source paths, crops, or export locations. |
| **Export** | Enable plain text, ANSI colour, or both in the toolbar. Use `Ctrl+S` to export the selected image, or choose **Export all** for batch output. |

Interactive edits are converted in the background, so the preview stays responsive. When a newer edit arrives, an older preview job is discarded rather than replacing your latest result.

## Rust desktop exported files

- Plain `.txt` files contain printable ASCII characters and LF line endings.
- ANSI `.ansi.txt` files use true-colour foreground sequences in the form `ESC[38;2;R;G;Bm`, avoid unnecessary colour changes, and reset styling at every line boundary and at the end of the file.
- ANSI art needs a true-colour-capable terminal and a monospace font. A normal text editor will show its escape sequences instead of colours.
- Single-image export opens a save dialog. Batch export asks for a folder and uses names such as `photo_ascii.txt` and `photo_ascii.ansi.txt`.
- Exports are written through a temporary sibling file before replacement, so an interrupted conversion does not leave a partial output file behind.

## Web and WebAssembly application

### Browser architecture

The browser edition is a static HTML/CSS/JavaScript application rather than a second Rust GUI. Browser-native APIs are better suited to file picking, drag and drop, image decoding, clipboard permissions, responsive layout, and downloads, while the Rust library remains responsible for the conversion itself. There is no runtime framework and no JavaScript bundler.

| Web file | Responsibility |
| --- | --- |
| `web/index.html` | Defines the accessible toolbar, source area, conversion controls, live status, ASCII preview, and copy/download actions. |
| `web/styles.css` | Recreates the desktop editor's dark panel layout, cyan accent, source viewport, settings sidebar, preview region, and mobile single-column layout. |
| `web/app.js` | Handles files, browser decoding, downscaling, settings validation, debouncing, worker coordination, stale-result protection, preview fitting, copying, and downloads. |
| `web/worker.js` | Loads the Wasm module, owns the cached Rust `AsciiImage`, and performs conversions away from the main browser thread. |
| `web-wasm/src/lib.rs` | Validates raw RGBA dimensions and exposes the Rust converter to JavaScript through `wasm-bindgen`. |
| `scripts/build-web.mjs` | Produces `dist/web` by copying static assets and running `wasm-pack --target web`. |
| `scripts/serve-web.mjs` | Serves the production directory locally with correct MIME types and path-containment checks. |
| `tests/web/app.spec.js` | Exercises the finished app through Chromium, Firefox, and WebKit. |

The `ascii-art-generator-web` crate is compiled as both `cdylib` and `rlib`. Its JavaScript-facing constructor is `new AsciiImage(width, height, rgba)`. It verifies non-zero dimensions, checks multiplication overflow, confirms that the byte array contains exactly `width × height × 4` values, and then stores an `image::RgbaImage` inside Wasm. `render` provides the minimal columns/ramp API, while `render_adjusted` adds brightness, contrast, gamma, saturation, RGB gains, and matte colour. Rust validation failures are returned as readable JavaScript exceptions.

### Image and conversion flow

The browser data flow is intentionally explicit:

1. `worker.js` imports the browser-targeted Wasm ES module and reports either a ready or fatal state through `postMessage`.
2. A selected or dropped file is decoded on the main thread with `createImageBitmap`. The app first requests EXIF-aware orientation and falls back to the simpler call when a browser does not support that option. Animated formats use the bitmap's decoded first frame.
3. If the source is larger than 4096 pixels on either edge, a canvas reduces it while preserving aspect ratio. `getImageData` produces the RGBA buffer required by the Rust adapter.
4. The RGBA `ArrayBuffer` is transferred—not copied—to the module worker. The worker frees any previous `AsciiImage`, constructs the new one once, and keeps it in Wasm for later setting changes.
5. JavaScript validates the 10–400 column limit, the 2–256 printable-character ramp, all tone ranges, and the matte value before scheduling work. Valid changes are debounced for 160 milliseconds.
6. The worker calls `render_adjusted` with the full image, automatic rows at the `0.5` cell ratio, no dithering, and the selected tone values. The returned plain string is already LF-terminated.
7. The main thread assigns the string with `textContent`, enabling copy and download only when the newest image/settings combination has completed successfully.

Only one conversion is active at a time. While it is running, the main thread retains one pending settings object and replaces that object whenever a newer edit arrives. Image IDs, request IDs, and settings generation numbers are checked on every response. This coalesces rapid slider input and prevents results from an old image or old settings from appearing in the preview.

The preview uses a `<pre>` element so spaces and line endings are preserved without injecting HTML. A `ResizeObserver` measures the available width and scales the monospace font down from 12 pixels when the output can still remain readable; at the 3-pixel floor the container switches naturally to horizontal scrolling. The desktop-like two-column grid becomes source, settings, then preview on narrow screens.

Copy uses `navigator.clipboard.writeText`. If browser permissions or an insecure origin block it, the app reports the error in its live status region and selects the preview for manual copying. Download creates an LF-terminated UTF-8 `Blob` with `text/plain;charset=utf-8`, derives a safe `<source>_ascii.txt` filename, clicks a temporary link, and revokes its object URL afterward.

### Static build and browser tests

`npm run build:web` removes only the expected `dist/web` output directory, copies the authored HTML, CSS, JavaScript, README, and license, then invokes `wasm-pack build web-wasm --target web --release`. The result is native browser ES modules plus the `.wasm` binary in `dist/web/pkg`; no development server or Node runtime is required in production. The included local server exists because Wasm modules must be fetched over HTTP with the correct MIME type rather than opened with a `file:` URL.

Rust adapter tests cover invalid dimensions and RGBA lengths, invalid settings, black/white determinism, transparency mattes, line endings, and parity with a direct core conversion. Playwright builds the production directory and tests Wasm startup, picker and drag/drop input, setting changes, stale-response suppression, failures, responsive layout, clipboard behavior, and downloaded file contents in Chromium, Firefox, and WebKit.

## Web app and future widget direction

The browser-facing version is implemented as the standalone static web app described above. Visitors can drop in an image, adjust its character ramp, size, tone, colour gains, and transparency matte, preview the result in a responsive `<pre>` block, then copy or download the text.

The Rust conversion engine is compiled through a small WebAssembly adapter while browser APIs handle image loading and downloads. Unlike the desktop editor, the web app deliberately omits cropping, image queues, dithering, ANSI-colour export, presets, and batch export. A reusable custom element or npm package with a stable embedding API is still future work; this release intentionally provides a standalone app and copyable static assets instead.

Other possible future additions include animation export, editable project files, and richer colour-profile support.

## Development

```shell
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo build --release
wasm-pack test --node web-wasm
npm run test:web
```

Install the Playwright browsers once with `npx playwright install chromium firefox webkit` before running the web test suite locally. `npm run test:web` rebuilds `dist/web`, starts a local server, and exercises the app in all three browser engines.

The conversion engine is available independently of the GUI through the `ascii_art_generator` library. Its public API includes `ConversionSettings`, `CropRect`, `CharacterRamp`, `DitherMode`, `RowSizing`, `AsciiCell`, `AsciiDocument`, `convert`, `render_plain`, and `render_ansi`. Native file decoding and filesystem export are part of the default `desktop` feature; use `default-features = false` for the browser-safe conversion core.

GitHub Actions checks formatting, Clippy, native and WebAssembly tests, and the browser suite. It produces a copyable `ascii-art-generator-web` static-site artifact plus unsigned portable desktop artifacts for Windows x86-64, Linux x86-64, macOS Apple Silicon, and macOS Intel. macOS artifacts are not signed or notarized.

## License

MIT
