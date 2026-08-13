// Renders the tray and window mark PNGs from src-tauri/icons/icon.svg.
// The Rust build embeds these with include_bytes!, so the outputs are
// committed; run `pnpm render:icons` after changing the mark. The installed
// app icon set is separate: `pnpm tauri icon src-tauri/icons/icon-app.svg`.
//
// Variants: the bare mark in white (dark taskbars) and black (light
// taskbars) at 32 px for the tray and 64 px for the window. The recording
// state needs no asset of its own; the white mark is tinted at runtime.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";

const iconsDir = join(
    dirname(fileURLToPath(import.meta.url)),
    "..",
    "src-tauri",
    "icons",
);
const markSvg = readFileSync(join(iconsDir, "icon.svg"), "utf8");

function variant(color) {
    return markSvg.replaceAll('stroke="white"', `stroke="${color}"`);
}

function render(svg, size, name) {
    const png = new Resvg(svg, { fitTo: { mode: "width", value: size } })
        .render()
        .asPng();
    writeFileSync(join(iconsDir, name), png);
    console.log(`${name} (${size}x${size}, ${png.length} bytes)`);
}

for (const color of ["white", "black"]) {
    render(variant(color), 32, `mark-${color}-32.png`);
    render(variant(color), 64, `mark-${color}-64.png`);
}
