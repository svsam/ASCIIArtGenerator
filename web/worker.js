import init, { AsciiImage } from "./pkg/ascii_art_generator_web.js";

let sourceImage = null;
let activeImageId = 0;

const wasmReady = init()
  .then(() => {
    self.postMessage({ type: "ready" });
  })
  .catch((error) => {
    self.postMessage({ type: "fatal", message: readableError(error) });
    throw error;
  });

self.addEventListener("message", async (event) => {
  const message = event.data;
  try {
    await wasmReady;
    if (message.type === "set-image") {
      sourceImage?.free();
      const pixels = new Uint8Array(
        message.pixels.buffer,
        message.pixels.byteOffset,
        message.pixels.byteLength,
      );
      sourceImage = new AsciiImage(message.width, message.height, pixels);
      activeImageId = message.imageId;
      self.postMessage({ type: "image-ready", imageId: activeImageId });
      return;
    }

    if (message.type === "convert") {
      if (!sourceImage || message.imageId !== activeImageId) {
        throw new Error("The source image is no longer available.");
      }
      const text = sourceImage.render_adjusted(
        message.columns,
        message.ramp,
        message.brightness,
        message.contrast,
        message.gamma,
        message.saturation,
        message.redGain,
        message.greenGain,
        message.blueGain,
        message.matte[0],
        message.matte[1],
        message.matte[2],
      );
      self.postMessage({
        type: "result",
        requestId: message.requestId,
        imageId: message.imageId,
        text,
      });
    }
  } catch (error) {
    self.postMessage({
      type: "error",
      requestId: message.requestId,
      imageId: message.imageId,
      message: readableError(error),
    });
  }
});

function readableError(error) {
  if (error instanceof Error && error.message) {
    return error.message;
  }
  return String(error || "Unknown conversion error");
}
