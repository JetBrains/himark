// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
// Render the original Air CSS against the same DOM/content as the native fixtures.
// Usage: node scripts/air-ui-reference.cjs JCP_FRONTEND_CHECKOUT TOOL_DIRECTORY OUTPUT_DIRECTORY
const fs = require("fs"),
  path = require("path");
const [checkout, toolDirectory, out] = process.argv.slice(2);
if (!out)
  throw new Error("Expected checkout, tool directory and output directory");
const load = require("module").createRequire(
  path.resolve(toolDirectory, "package.json"),
);
const sass = load("sass");
const kit = path.resolve(checkout, "libs/ui-kits/air-ui");
fs.mkdirSync(out, { recursive: true });
const sections = [
  "colors/primitives",
  "colors/light",
  "colors/dark",
  "typography/main",
  "typography/light",
  "typography/dark",
  "spacings/default",
  "spacings/semantic",
];
let css = sections
  .map(
    (file) => sass.compile(`${kit}/resources/styles/tokens/${file}.scss`).css,
  )
  .join("\n");
for (const type of [
  "button",
  "button-ghost",
  "checkbox",
  "text",
  "list",
  "card",
]) {
  const componentCss = sass.compile(
    `${kit}/src/components/${type}/${type}.module.scss`,
    { loadPaths: [kit] },
  ).css;
  // Scope CSS modules without changing the component styles.
  css += componentCss.replace(/\.([a-zA-Z][\w-]*)/g, (_, c) => `.${type}-${c}`);
}
for (const [family, file] of [
  ["Inter", "inter/InterVariable-latin.woff2"],
  ["JetBrains Mono", "jb-mono/JetBrainsMonoVar-latin.woff2"],
]) {
  css += `@font-face{font-family:'${family}';font-weight:100 900;src:url(data:font/woff2;base64,${fs.readFileSync(`${kit}/resources/fonts/${file}`).toString("base64")}) format('woff2');}`;
}
css = css
  .replaceAll(":hover", ".hovered")
  .replaceAll(":active", ".pressed")
  .replaceAll(":focus-visible", ".focused");
css +=
  "\n*{box-sizing:border-box}body{margin:0;background:var(--air-background)}.fixture{position:absolute}.text-secondary{color:var(--air-text-secondary)}.text-primary{color:var(--air-text-primary)}";
const fixtures = [];
const add = (id, x, y, html) => fixtures.push({ id, x, y, html });
let y = 20;
for (const [variant, label] of [
  ["h2-semibold", "Heading — a section title"],
  ["default", "Label — the quick brown fox"],
  ["medium", "Caption — secondary information"],
  ["h5-semibold", "CAPS — TRACKED SECTION LABEL"],
  ["small", "Key hint — Ctrl+Shift+P"],
  ["code", "Code — fn main() {}"],
]) {
  add(
    `text-${variant}`,
    20,
    y,
    `<div class="text-${variant} text-${["h2-semibold", "default", "code"].includes(variant) ? "primary" : "secondary"}">${label}</div>`,
  );
  y += 32;
}
const states = ["default", "hovered", "pressed", "focused", "disabled"];
for (const [row, role] of ["primary", "secondary", "ghost"].entries())
  for (const [col, state] of states.entries()) {
    const prefix = role === "ghost" ? "button-ghost" : "button";
    const label = role[0].toUpperCase() + role.slice(1);
    add(
      `${role}-${state}`,
      20 + col * 130,
      240 + row * 60,
      `<button class="${prefix}-button ${prefix}-${role} ${state}" ${state === "disabled" ? "disabled" : ""}><span class="${prefix}-content"><span class="${prefix}-wrap">${label}</span></span></button>`,
    );
  }
for (const [row, value] of ["unchecked", "checked", "indeterminate"].entries())
  for (const [col, state] of states.entries()) {
    const icon =
      value === "unchecked"
        ? ""
        : fs.readFileSync(
            `${kit}/resources/icons/checkbox-${value}.svg`,
            "utf8",
          );
    add(
      `checkbox-${value}-${state}`,
      20 + col * 130,
      440 + row * 44,
      `<button class="checkbox-checkbox ${state}" data-state="${value}" ${state === "disabled" ? "disabled" : ""}><span class="checkbox-indicator">${icon}</span></button>`,
    );
  }
add(
  "list-default",
  20,
  600,
  '<div class="list-root" style="width:600px"><div></div><div class="list-mainRow">Standard</div><span class="text-small text-secondary">Metadata</span></div>',
);
for (const [col, kind] of ["fill", "bordered", "outline"].entries()) {
  const fill =
    kind === "outline"
      ? "transparent"
      : "var(--air-card-background-default-default)";
  const border = kind === "fill" ? "transparent" : "var(--air-border)";
  add(
    `surface-${kind}`,
    20 + col * 210,
    660,
    `<div class="text-default text-primary" style="width:190px;padding:11px;border:1px solid ${border};border-radius:8px;background:${fill}">${kind[0].toUpperCase() + kind.slice(1)}</div>`,
  );
}
const html = `<!doctype html><html data-air-theme="default-dark"><head><meta charset="utf-8"><style>${css}</style></head><body>${fixtures.map((f) => `<div id="${f.id}" class="fixture" style="left:${f.x}px;top:${f.y}px">${f.html}</div>`).join("")}</body></html>`;
fs.writeFileSync(path.join(out, "air-ui-reference.html"), html);
fs.writeFileSync(
  path.join(out, "air-ui-fixtures.json"),
  JSON.stringify(fixtures, null, 2),
);
(async () => {
  const { chromium: pw } = load("playwright");
  const browser = await pw.launch({
    executablePath: process.env.AIR_CHROMIUM_PATH,
    headless: true,
    args: [
      "--no-sandbox",
      "--disable-dev-shm-usage",
      "--font-render-hinting=none",
    ],
  });
  const page = await browser.newPage({
    viewport: { width: 700, height: 740 },
    deviceScaleFactor: 1,
  });
  await page.setContent(html);
  await page.evaluate(() => document.fonts.ready);
  for (const mode of ["dark", "light"]) {
    await page
      .locator("html")
      .evaluate((el, mode) => (el.dataset.airTheme = `default-${mode}`), mode);
    await page.screenshot({ path: path.join(out, `air-web-${mode}.png`) });
    const metrics = await page.locator(".fixture").evaluateAll((items) =>
      items.map((el) => {
        const c = el.firstElementChild,
          r = c.getBoundingClientRect(),
          s = getComputedStyle(c);
        let baseline;
        if (el.id.startsWith("text-")) {
          const marker = document.createElement("span");
          marker.style =
            "display:inline-block;width:0;height:0;vertical-align:baseline";
          c.append(marker);
          baseline = marker.getBoundingClientRect().y - r.y;
          marker.remove();
        }
        return {
          id: el.id,
          x: r.x,
          y: r.y,
          width: r.width,
          height: r.height,
          baseline,
          font: s.font,
          letterSpacing: s.letterSpacing,
          color: s.color,
          background: s.backgroundColor,
        };
      }),
    );
    fs.writeFileSync(
      path.join(out, `air-web-${mode}-metrics.json`),
      JSON.stringify(metrics, null, 2),
    );
  }
  await browser.close();
  console.log("Rendered web reference fixtures.");
})();
