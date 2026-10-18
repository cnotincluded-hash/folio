// Builds dist/ for the desktop app from web/folio.html:
// wraps the page in a full HTML document and swaps the Google Fonts link for
// bundled copies of Inter, so the app works offline. (On a Mac the page uses
// the system font first; Windows uses Inter.)
import { readFileSync, writeFileSync, mkdirSync, copyFileSync, rmSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const dist = join(root, 'dist');
const fontsOut = join(dist, 'fonts');
rmSync(dist, { recursive: true, force: true });
mkdirSync(fontsOut, { recursive: true });

const FONTS = [{ pkg: 'inter', weights: [400, 500, 600, 700] }];
const SUBSETS = ['latin', 'latin-ext'];

let fontCss = '';
for (const { pkg, weights } of FONTS) {
  const base = join(root, 'node_modules', '@fontsource', pkg);
  if (!existsSync(base)) throw new Error(`Missing @fontsource/${pkg}. Run "npm install" first.`);
  for (const w of weights) {
    const css = readFileSync(join(base, `${w}.css`), 'utf8');
    for (const block of css.split('@font-face').slice(1)) {
      const name = /url\(\.\/files\/([^)]+?\.woff2)\)/.exec(block);
      if (!name || !SUBSETS.some(s => name[1].includes(`-${s}-${w}-`))) continue;
      copyFileSync(join(base, 'files', name[1]), join(fontsOut, name[1]));
      const range = /unicode-range:\s*([^;]+);/.exec(block);
      const family = /font-family:\s*'([^']+)'/.exec(block)[1];
      fontCss += `@font-face{font-family:'${family}';font-style:normal;font-display:swap;font-weight:${w};src:url(fonts/${name[1]}) format('woff2');${range ? `unicode-range:${range[1]};` : ''}}\n`;
    }
  }
  copyFileSync(join(base, 'LICENSE'), join(fontsOut, `${pkg}-LICENSE.txt`));
}

let page = readFileSync(join(root, 'web', 'folio.html'), 'utf8');
const fontLinks = /<link rel="preconnect"[^>]*>\s*|<link rel="stylesheet" href="https:\/\/fonts\.googleapis\.com[^>]*>\s*/g;
if (!fontLinks.test(page)) throw new Error('Could not find the Google Fonts links in web/folio.html');
page = page.replace(fontLinks, '');

const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<style>
${fontCss}:root{color-scheme:light}
body{margin:0;font:14px system-ui,sans-serif}
img{max-width:100%}
[hidden]{display:none!important}
</style>
</head>
<body>
${page}
</body>
</html>
`;
writeFileSync(join(dist, 'index.html'), html);
console.log(`dist/index.html written (${Math.round(html.length / 1024)} KB, ${fontCss.split('\n').length - 1} font faces)`);
