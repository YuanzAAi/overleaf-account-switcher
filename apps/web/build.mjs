import { build, context } from "esbuild";
import { copyFile, mkdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createServer } from "node:http";
import { request } from "node:http";

process.chdir(fileURLToPath(new URL(".", import.meta.url)));
await mkdir("dist", { recursive: true });
await copyFile("index.html", "dist/index.html");
const options = {
  entryPoints: { app: "src/app.js", styles: "styles.css" },
  bundle: true,
  outdir: "dist",
  target: "es2020",
  format: "iife",
  loader: { ".woff2": "dataurl", ".png": "dataurl" },
  legalComments: "eof",
  minify: !process.argv.includes("--serve"),
};
if (!process.argv.includes("--serve")) {
  await build(options);
} else {
  const ctx = await context(options);
  await ctx.watch();
  const server = createServer(async (req, res) => {
    const pathname = new URL(req.url, "http://localhost").pathname;
    if (pathname.startsWith("/ui") || pathname === "/") {
      const name = pathname.endsWith(".js")
        ? "app.js"
        : pathname.endsWith(".css")
          ? "styles.css"
          : "index.html";
      try {
        res.setHeader(
          "Content-Type",
          name.endsWith(".js") ? "text/javascript" : name.endsWith(".css") ? "text/css" : "text/html",
        );
        res.end(await readFile(name === "index.html" ? name : `dist/${name}`));
      } catch {
        res.writeHead(503);
        res.end("Building");
      }
      return;
    }
    const upstream = request(
      {
        hostname: "127.0.0.1",
        port: process.env.SERVICE_PORT || 8765,
        path: req.url,
        method: req.method,
        headers: req.headers,
      },
      (incoming) => {
        res.writeHead(incoming.statusCode, incoming.headers);
        incoming.pipe(res);
      },
    );
    upstream.on("error", () => {
      if (!res.headersSent) res.writeHead(503);
      res.end();
    });
    res.on("close", () => upstream.destroy());
    req.pipe(upstream);
  });
  server.listen(Number(process.env.PORT || 5173), "127.0.0.1", () =>
    console.log(`http://127.0.0.1:${server.address().port}/ui/`),
  );
}
