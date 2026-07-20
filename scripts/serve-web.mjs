import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const webRoot = join(repositoryRoot, "dist", "web");
const port = Number.parseInt(process.env.ASCII_ART_PORT ?? "4173", 10);
const host = "127.0.0.1";

if (!existsSync(join(webRoot, "index.html"))) {
  console.error("No web build found. Run `npm run build:web` first.");
  process.exit(1);
}

const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".md", "text/markdown; charset=utf-8"],
  [".txt", "text/plain; charset=utf-8"],
  [".wasm", "application/wasm"],
]);

const server = createServer((request, response) => {
  try {
    const url = new URL(request.url ?? "/", `http://${host}:${port}`);
    const requestedPath = decodeURIComponent(url.pathname === "/" ? "/index.html" : url.pathname);
    const relativePath = normalize(requestedPath).replace(/^([/\\])+/, "");
    let filePath = resolve(webRoot, relativePath);
    if (filePath !== webRoot && !filePath.startsWith(`${webRoot}${sep}`)) {
      response.writeHead(403).end("Forbidden");
      return;
    }
    if (existsSync(filePath) && statSync(filePath).isDirectory()) {
      filePath = join(filePath, "index.html");
    }
    if (!existsSync(filePath) || !statSync(filePath).isFile()) {
      response.writeHead(404).end("Not found");
      return;
    }

    response.writeHead(200, {
      "Cache-Control": "no-store",
      "Content-Type": contentTypes.get(extname(filePath)) ?? "application/octet-stream",
    });
    createReadStream(filePath).pipe(response);
  } catch (error) {
    response.writeHead(500).end("Server error");
    console.error(error);
  }
});

server.listen(port, host, () => {
  console.log(`ASCII Art Generator web app: http://${host}:${port}`);
});
