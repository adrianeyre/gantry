// Draws Gantry's icons and its link-preview image (og-image.jpg) into web/public.
//
//   pnpm --filter @gantry/web brand
//
// The SVG favicon is the source drawing; every PNG is rendered from it (or,
// for the social card, from the HTML below) with Playwright's Chromium, and
// favicon.ico packs the 16, 32 and 48 pixel PNGs. Run it again after changing
// the mark or the card, and commit what it writes.
import { mkdir, writeFile } from "node:fs/promises";
import { chromium } from "@playwright/test";

const out = new URL("../public/", import.meta.url);

/** Cobalt, the default theme's accent, and its darker partner. */
const BRAND = "#1d4ed8";
const BRAND_DEEP = "#13338c";

/**
 * The mark: a printer's gantry (two uprights and a crossbar) with the print
 * head riding on it, laying down a first line. `inset` shrinks it into the
 * middle of the tile, for maskable icons that platforms crop to a circle.
 */
function mark({ inset = 0, rounded = true } = {}) {
  const scale = 1 - inset * 2;
  const shift = 64 * inset;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${BRAND}"/><stop offset="1" stop-color="${BRAND_DEEP}"/></linearGradient></defs>
  <rect width="64" height="64" rx="${rounded ? 14 : 0}" fill="url(#g)"/>
  <g transform="translate(${shift} ${shift}) scale(${scale})" fill="none" stroke="#fff" stroke-linecap="round" stroke-linejoin="round">
    <path d="M14 50V16h36v34" stroke-width="5"/>
    <rect x="25" y="12" width="14" height="13" rx="3" fill="#fff" stroke="none"/>
    <path d="M32 25v7" stroke-width="4"/>
    <path d="M21 44h22" stroke-width="5"/>
  </g>
</svg>
`;
}

/** The 1200×630 card social sites show beside a shared link. */
function socialCard() {
  const printer = (name, state, progress, colour) => `
    <div class="card">
      <div class="row"><b>${name}</b><span class="badge"><i></i>Connected</span></div>
      <div class="state">${state}</div>
      <div class="bar"><div style="width:${progress}%;background:${colour}"></div></div>
      <div class="facts"><span>Nozzle 220 °C</span><span>Bed 60 °C</span><span>${progress}%</span></div>
    </div>`;
  return `<!doctype html><html><head><style>
  * { box-sizing: border-box; margin: 0; }
  body { width: 1200px; height: 630px; overflow: hidden; font-family: "Segoe UI", system-ui, sans-serif;
    background: radial-gradient(circle at 85% 10%, #22346f 0, transparent 55%), linear-gradient(135deg, #0b1020, #111a33);
    color: #e9edf3; display: grid; grid-template-columns: 560px 1fr; gap: 48px; padding: 72px 72px 64px; }
  .brand { display: flex; align-items: center; gap: 20px; font-size: 44px; font-weight: 700; letter-spacing: -0.5px; }
  .brand img { width: 84px; height: 84px; }
  h1 { font-size: 64px; line-height: 1.05; font-weight: 750; letter-spacing: -1.5px; margin-top: 56px; }
  p { font-size: 26px; line-height: 1.4; color: #b4bfd3; margin-top: 24px; }
  .tags { display: flex; gap: 12px; margin-top: 36px; flex-wrap: wrap; }
  .tags span { border: 1px solid #3a4a75; border-radius: 99px; padding: 6px 16px; font-size: 20px; color: #cfd8ea; }
  .cards { display: grid; gap: 18px; align-content: center; }
  .card { background: #161e33; border: 1px solid #2c3a5e; border-radius: 18px; padding: 20px 24px; box-shadow: 0 18px 40px rgb(0 0 0 / .35); }
  .card:nth-child(2) { transform: translateX(-28px); }
  .row { display: flex; justify-content: space-between; align-items: center; font-size: 22px; }
  .badge { display: inline-flex; gap: 8px; align-items: center; color: #5fd38d; border: 1px solid #5fd38d; border-radius: 99px; padding: 2px 12px; font-size: 16px; font-weight: 600; }
  .badge i { width: 8px; height: 8px; border-radius: 50%; background: #5fd38d; }
  .state { margin-top: 8px; font-size: 20px; font-weight: 600; color: #cfd8ea; }
  .bar { margin-top: 12px; height: 12px; border-radius: 99px; background: #26314f; overflow: hidden; }
  .bar div { height: 100%; border-radius: 99px; }
  .facts { display: flex; gap: 20px; margin-top: 10px; font-size: 16px; color: #9aa7c0; }
  </style></head><body>
  <div>
    <div class="brand"><img src="data:image/svg+xml,${encodeURIComponent(mark())}" alt="">Gantry</div>
    <h1>Every 3D printer, one screen.</h1>
    <p>Self-hosted fleet dashboard: live status, cameras, Jobs and alerts, on your own network.</p>
    <div class="tags"><span>Live status</span><span>Cameras</span><span>Jobs</span><span>Alerts</span></div>
  </div>
  <div class="cards">
    ${printer("Workshop X1C", "Printing · Layer 142 of 310", 46, "#7ea6ff")}
    ${printer("Garage P1S", "Printing · 1 h 12 m left", 78, "#4fd6c8")}
    ${printer("Office A1 mini", "Printing · Layer 12 of 96", 13, "#f7a463")}
  </div>
  </body></html>`;
}

/** An ICO file holding the given PNGs, which every browser since Vista reads. */
function ico(images) {
  const header = Buffer.alloc(6 + images.length * 16);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(images.length, 4);
  let offset = header.length;
  images.forEach(({ size, png }, i) => {
    const at = 6 + i * 16;
    header.writeUInt8(size >= 256 ? 0 : size, at);
    header.writeUInt8(size >= 256 ? 0 : size, at + 1);
    header.writeUInt16LE(1, at + 4);
    header.writeUInt16LE(32, at + 6);
    header.writeUInt32LE(png.length, at + 8);
    header.writeUInt32LE(offset, at + 12);
    offset += png.length;
  });
  return Buffer.concat([header, ...images.map((i) => i.png)]);
}

async function main() {
  await mkdir(out, { recursive: true });
  const browser = await chromium.launch();
  const page = await browser.newPage();

  const render = async (svg, size) => {
    await page.setViewportSize({ width: size, height: size });
    await page.setContent(
      `<style>html,body{margin:0;background:transparent}img{display:block;width:${size}px;height:${size}px}</style>` +
        `<img src="data:image/svg+xml,${encodeURIComponent(svg)}">`,
    );
    return page.screenshot({ omitBackground: true });
  };

  await writeFile(new URL("favicon.svg", out), mark());
  const icoSizes = [];
  for (const size of [16, 32, 48]) icoSizes.push({ size, png: await render(mark(), size) });
  await writeFile(new URL("favicon.ico", out), ico(icoSizes));
  // Home-screen icons are drawn on a square: the platform rounds the corners itself.
  await writeFile(new URL("apple-touch-icon.png", out), await render(mark({ rounded: false, inset: 0.06 }), 180));
  await writeFile(new URL("icon-192.png", out), await render(mark(), 192));
  await writeFile(new URL("icon-512.png", out), await render(mark(), 512));
  await writeFile(new URL("icon-maskable-512.png", out), await render(mark({ rounded: false, inset: 0.12 }), 512));

  await page.setViewportSize({ width: 1200, height: 630 });
  await page.setContent(socialCard(), { waitUntil: "load" });
  // JPEG keeps it well under WhatsApp's ~300 KB preview limit; the others allow far more.
  await writeFile(new URL("og-image.jpg", out), await page.screenshot({ type: "jpeg", quality: 88 }));

  await browser.close();
  console.log("Wrote the icons and og-image.jpg to web/public");
}

await main();
