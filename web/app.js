const CLASSIC_RAMP = "@%#*+=-:. ";
const MIN_COLUMNS = 10;
const MAX_COLUMNS = 400;
const MAX_SOURCE_EDGE = 4096;
const CONVERSION_DEBOUNCE_MS = 160;

const elements = {
  fileInput: document.querySelector("#file-input"),
  dropZone: document.querySelector("#drop-zone"),
  sourcePreviewFrame: document.querySelector("#source-preview-frame"),
  sourcePreview: document.querySelector("#source-preview"),
  sourceName: document.querySelector("#source-name"),
  sourceDimensions: document.querySelector("#source-dimensions"),
  rampInput: document.querySelector("#ramp-input"),
  columnsRange: document.querySelector("#columns-range"),
  columnsNumber: document.querySelector("#columns-number"),
  settingsError: document.querySelector("#settings-error"),
  previewShell: document.querySelector("#preview-shell"),
  output: document.querySelector("#ascii-output"),
  outputDimensions: document.querySelector("#output-dimensions"),
  copyButton: document.querySelector("#copy-button"),
  downloadButton: document.querySelector("#download-button"),
  status: document.querySelector("#status"),
};

const state = {
  workerReady: false,
  workerImageReady: false,
  imageId: 0,
  requestId: 0,
  settingsVersion: 0,
  activeRequest: null,
  pendingSettings: null,
  debounceTimer: null,
  output: "",
  outputColumns: 0,
  sourceFilename: "image",
  sourceObjectUrl: null,
};

elements.rampInput.value = CLASSIC_RAMP;

const worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
worker.addEventListener("message", handleWorkerMessage);
worker.addEventListener("error", () => {
  setStatus("The converter worker stopped unexpectedly. Reload the page to try again.", "error");
  invalidateOutput();
});

elements.dropZone.addEventListener("click", () => elements.fileInput.click());
elements.fileInput.addEventListener("change", () => {
  void loadFiles(elements.fileInput.files);
  elements.fileInput.value = "";
});

for (const eventName of ["dragenter", "dragover"]) {
  elements.dropZone.addEventListener(eventName, (event) => {
    event.preventDefault();
    if (event.dataTransfer) {
      event.dataTransfer.dropEffect = "copy";
    }
    elements.dropZone.classList.add("is-dragging");
  });
}
for (const eventName of ["dragleave", "drop"]) {
  elements.dropZone.addEventListener(eventName, (event) => {
    event.preventDefault();
    elements.dropZone.classList.remove("is-dragging");
  });
}
elements.dropZone.addEventListener("drop", (event) => {
  void loadFiles(event.dataTransfer?.files);
});

elements.rampInput.addEventListener("input", settingsChanged);
elements.columnsRange.addEventListener("input", () => {
  elements.columnsNumber.value = elements.columnsRange.value;
  settingsChanged();
});
elements.columnsNumber.addEventListener("input", () => {
  const columns = Number(elements.columnsNumber.value);
  if (Number.isInteger(columns) && columns >= MIN_COLUMNS && columns <= MAX_COLUMNS) {
    elements.columnsRange.value = String(columns);
  }
  settingsChanged();
});
elements.copyButton.addEventListener("click", copyOutput);
elements.downloadButton.addEventListener("click", downloadOutput);

new ResizeObserver(fitPreview).observe(elements.previewShell);

async function loadFiles(fileList) {
  const files = Array.from(fileList ?? []);
  if (files.length === 0) {
    return;
  }
  const file = files.find((candidate) => candidate.type.startsWith("image/")) ?? files[0];
  const imageId = ++state.imageId;
  state.workerImageReady = false;
  state.activeRequest = null;
  state.pendingSettings = null;
  clearTimeout(state.debounceTimer);
  clearSourcePreview();
  invalidateOutput(true);
  setStatus("Decoding image in your browser…");

  try {
    const bitmap = await decodeBitmap(file);
    if (imageId !== state.imageId) {
      bitmap.close();
      return;
    }

    const originalWidth = bitmap.width;
    const originalHeight = bitmap.height;
    const scale = Math.min(1, MAX_SOURCE_EDGE / Math.max(originalWidth, originalHeight));
    const width = Math.max(1, Math.round(originalWidth * scale));
    const height = Math.max(1, Math.round(originalHeight * scale));
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) {
      throw new Error("This browser could not create an image canvas.");
    }
    context.drawImage(bitmap, 0, 0, width, height);
    bitmap.close();
    const imageData = context.getImageData(0, 0, width, height);

    state.sourceFilename = file.name || "image";
    updateSourcePreview(file, originalWidth, originalHeight, width, height);
    setStatus(state.workerReady ? "Preparing conversion…" : "Waiting for the converter…");
    worker.postMessage(
      {
        type: "set-image",
        imageId,
        width,
        height,
        pixels: imageData.data,
      },
      [imageData.data.buffer],
    );
  } catch (error) {
    if (imageId !== state.imageId) {
      return;
    }
    setStatus(`Could not decode that image. ${readableError(error)}`, "error");
    state.workerImageReady = false;
    invalidateOutput();
  }
}

async function decodeBitmap(file) {
  try {
    return await createImageBitmap(file, { imageOrientation: "from-image" });
  } catch (firstError) {
    try {
      return await createImageBitmap(file);
    } catch {
      throw firstError;
    }
  }
}

function updateSourcePreview(file, originalWidth, originalHeight, conversionWidth, conversionHeight) {
  state.sourceObjectUrl = URL.createObjectURL(file);
  elements.sourcePreview.src = state.sourceObjectUrl;
  elements.sourcePreview.alt = `Selected source: ${file.name || "image"}`;
  elements.sourceName.textContent = file.name || "image";
  elements.sourcePreviewFrame.hidden = false;
  const resized = originalWidth !== conversionWidth || originalHeight !== conversionHeight;
  elements.sourceDimensions.textContent = resized
    ? `${originalWidth}×${originalHeight} · sampled at ${conversionWidth}×${conversionHeight}`
    : `${originalWidth}×${originalHeight}`;
}

function clearSourcePreview() {
  if (state.sourceObjectUrl) {
    URL.revokeObjectURL(state.sourceObjectUrl);
    state.sourceObjectUrl = null;
  }
  elements.sourcePreview.removeAttribute("src");
  elements.sourcePreviewFrame.hidden = true;
  elements.sourceName.textContent = "";
  elements.sourceDimensions.textContent = "";
}

function settingsChanged() {
  clearTimeout(state.debounceTimer);
  const settingsVersion = ++state.settingsVersion;
  const validation = readSettings();
  if (!validation.ok) {
    state.pendingSettings = null;
    elements.settingsError.textContent = validation.message;
    invalidateOutput(true);
    setStatus("Fix the conversion settings to update the preview.", "error");
    return;
  }

  elements.settingsError.textContent = "";
  invalidateOutput(true);
  if (!state.workerImageReady) {
    return;
  }
  state.debounceTimer = setTimeout(() => {
    queueConversion(validation.settings, settingsVersion);
  }, CONVERSION_DEBOUNCE_MS);
}

function readSettings() {
  const columns = Number(elements.columnsNumber.value);
  if (!Number.isInteger(columns) || columns < MIN_COLUMNS || columns > MAX_COLUMNS) {
    return {
      ok: false,
      message: `Choose a whole-number width from ${MIN_COLUMNS} to ${MAX_COLUMNS} columns.`,
    };
  }

  const ramp = elements.rampInput.value;
  const characters = Array.from(ramp);
  if (characters.length < 2 || characters.length > 256) {
    return { ok: false, message: "The character ramp must contain 2–256 characters." };
  }
  if (characters.some((character) => character.codePointAt(0) < 32 || character.codePointAt(0) > 126)) {
    return { ok: false, message: "The character ramp can use printable ASCII characters only." };
  }

  return { ok: true, settings: { columns, ramp } };
}

function queueConversion(settings, settingsVersion = state.settingsVersion) {
  state.pendingSettings = { ...settings, imageId: state.imageId, settingsVersion };
  invalidateOutput(true);
  if (!state.activeRequest) {
    sendPendingConversion();
  } else {
    setStatus("Conversion queued with your latest settings…");
  }
}

function sendPendingConversion() {
  if (!state.pendingSettings || !state.workerImageReady) {
    return;
  }
  const settings = state.pendingSettings;
  state.pendingSettings = null;
  const requestId = ++state.requestId;
  state.activeRequest = {
    requestId,
    imageId: settings.imageId,
    settingsVersion: settings.settingsVersion,
  };
  setStatus("Converting image…");
  worker.postMessage({
    type: "convert",
    requestId,
    imageId: settings.imageId,
    columns: settings.columns,
    ramp: settings.ramp,
  });
}

function handleWorkerMessage(event) {
  const message = event.data;
  if (message.type === "ready") {
    state.workerReady = true;
    if (state.imageId === 0) {
      setStatus("Choose an image to begin.");
    }
    return;
  }
  if (message.type === "fatal") {
    setStatus(`The WebAssembly converter could not start. ${message.message}`, "error");
    invalidateOutput();
    return;
  }
  if (message.type === "image-ready") {
    if (message.imageId !== state.imageId) {
      return;
    }
    state.workerImageReady = true;
    const validation = readSettings();
    if (validation.ok) {
      queueConversion(validation.settings);
    } else {
      elements.settingsError.textContent = validation.message;
    }
    return;
  }
  if (message.type !== "result" && message.type !== "error") {
    return;
  }

  const matchesActive =
    state.activeRequest &&
    state.activeRequest.requestId === message.requestId &&
    state.activeRequest.imageId === message.imageId;
  if (!matchesActive) {
    return;
  }
  const completedRequest = state.activeRequest;
  state.activeRequest = null;

  if (message.imageId !== state.imageId) {
    sendPendingConversion();
    return;
  }
  if (state.pendingSettings) {
    sendPendingConversion();
    return;
  }
  if (completedRequest.settingsVersion !== state.settingsVersion) {
    return;
  }
  if (message.type === "error") {
    setStatus(`Conversion failed. ${message.message}`, "error");
    invalidateOutput();
    return;
  }

  showOutput(message.text);
}

function showOutput(text) {
  state.output = text.endsWith("\n") ? text : `${text}\n`;
  state.outputColumns = state.output.indexOf("\n");
  elements.output.textContent = state.output;
  elements.output.classList.remove("is-stale");
  elements.previewShell.dataset.empty = "false";
  elements.copyButton.disabled = false;
  elements.downloadButton.disabled = false;
  const rows = state.output.split("\n").length - 1;
  elements.outputDimensions.textContent = `${state.outputColumns} × ${rows}`;
  setStatus("ASCII art ready.", "success");
  fitPreview();
}

function invalidateOutput(markStale = false) {
  elements.copyButton.disabled = true;
  elements.downloadButton.disabled = true;
  elements.output.classList.toggle("is-stale", markStale && Boolean(state.output));
  if (!markStale) {
    state.output = "";
    state.outputColumns = 0;
    elements.output.textContent = "";
    elements.outputDimensions.textContent = "";
    elements.previewShell.dataset.empty = "true";
  }
}

function fitPreview() {
  if (!state.outputColumns) {
    return;
  }
  const availableWidth = Math.max(1, elements.previewShell.clientWidth - 34);
  const measurement = document.createElement("canvas").getContext("2d");
  if (!measurement) {
    return;
  }
  measurement.font = '12px "Cascadia Mono", "SFMono-Regular", Consolas, monospace';
  const characterWidthAtTwelve = measurement.measureText("M").width || 7.2;
  const fitted = (availableWidth * 12) / (state.outputColumns * characterWidthAtTwelve);
  const fontSize = Math.max(3, Math.min(12, fitted));
  elements.output.style.setProperty("--preview-font-size", `${fontSize.toFixed(2)}px`);
}

async function copyOutput() {
  if (!state.output || elements.copyButton.disabled) {
    return;
  }
  try {
    await navigator.clipboard.writeText(state.output);
    setStatus("Copied ASCII art to the clipboard.", "success");
  } catch (error) {
    selectOutput();
    setStatus(
      `Clipboard access was blocked. The preview is selected so you can copy it manually. ${readableError(error)}`,
      "error",
    );
  }
}

function selectOutput() {
  const selection = window.getSelection();
  if (!selection) {
    return;
  }
  const range = document.createRange();
  range.selectNodeContents(elements.output);
  selection.removeAllRanges();
  selection.addRange(range);
  elements.output.focus();
}

function downloadOutput() {
  if (!state.output || elements.downloadButton.disabled) {
    return;
  }
  const blob = new Blob([state.output], { type: "text/plain;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = outputFilename(state.sourceFilename);
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
  setStatus("Downloaded ASCII art as a text file.", "success");
}

function outputFilename(sourceFilename) {
  const lastDot = sourceFilename.lastIndexOf(".");
  const rawStem = lastDot > 0 ? sourceFilename.slice(0, lastDot) : sourceFilename;
  const safeStem = rawStem
    .replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_")
    .replace(/[. ]+$/g, "")
    .trim();
  return `${safeStem || "image"}_ascii.txt`;
}

function setStatus(message, kind = "info") {
  elements.status.textContent = message;
  elements.status.dataset.kind = kind;
}

function readableError(error) {
  if (error instanceof Error && error.message) {
    return error.message;
  }
  return String(error || "Unknown error");
}
