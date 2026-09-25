// Renders design/icon.svg to design/icon.png (1024×1024), the source for
// `pnpm tauri icon`. Run with `pnpm icon` after editing the SVG.
import { readFileSync, writeFileSync } from "node:fs";
import { Resvg } from "@resvg/resvg-js";

const svg = readFileSync(new URL("../design/icon.svg", import.meta.url));
const png = new Resvg(svg, { fitTo: { mode: "width", value: 1024 } }).render().asPng();
writeFileSync(new URL("../design/icon.png", import.meta.url), png);
console.log(`design/icon.png: ${png.length} bytes`);
