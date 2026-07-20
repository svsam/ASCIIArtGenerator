import { cpSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { homedir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sourceDirectory = join(repositoryRoot, "web");
const outputDirectory = join(repositoryRoot, "dist", "web");
const wasmOutputDirectory = join(outputDirectory, "pkg");
const cargoWasmPack = join(homedir(), ".cargo", "bin", `wasm-pack${process.platform === "win32" ? ".exe" : ""}`);
const wasmPack = existsSync(cargoWasmPack) ? cargoWasmPack : "wasm-pack";

if (dirname(outputDirectory) !== join(repositoryRoot, "dist")) {
  throw new Error(`Refusing to replace unexpected output path: ${outputDirectory}`);
}

rmSync(outputDirectory, { recursive: true, force: true });
mkdirSync(outputDirectory, { recursive: true });

for (const asset of ["index.html", "styles.css", "app.js", "worker.js"]) {
  cpSync(join(sourceDirectory, asset), join(outputDirectory, asset));
}
cpSync(join(repositoryRoot, "LICENSE"), join(outputDirectory, "LICENSE"));
cpSync(join(repositoryRoot, "README.md"), join(outputDirectory, "README.md"));

const result = spawnSync(
  wasmPack,
  [
    "build",
    join(repositoryRoot, "web-wasm"),
    "--target",
    "web",
    "--release",
    "--out-dir",
    wasmOutputDirectory,
    "--out-name",
    "ascii_art_generator_web",
  ],
  {
    env: {
      ...process.env,
      PATH: `${dirname(cargoWasmPack)}${delimiter}${process.env.PATH ?? ""}`,
    },
    stdio: "inherit",
  },
);

if (result.error) {
  throw result.error;
}
if (result.status !== 0) {
  process.exitCode = result.status ?? 1;
} else {
  console.log(`Web build ready at ${outputDirectory}`);
}
