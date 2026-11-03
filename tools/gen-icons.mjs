// One-off build helper: turns the MIT-licensed Fluent System Icons SVGs into
// inline path markup for dist/api.js. Not a runtime dependency.
import { readFileSync, readdirSync } from "node:fs";

const SRC = process.argv[2];
const LIST = process.argv[3]; // "key=file-name key=file-name ..."

readdirSync(SRC);
const out = [];
for (const pair of LIST.split(/\s+/).filter(Boolean)) {
  const [key, name] = pair.split("=");
  const svg = readFileSync(`${SRC}/${name}.svg`, "utf8");
  const vb = svg.match(/viewBox="([^"]+)"/)[1];
  if (vb !== "0 0 20 20") throw new Error(`${name}: unexpected viewBox ${vb}`);
  const inner = svg.slice(svg.indexOf(">") + 1, svg.lastIndexOf("</svg>")).trim();
  if (!/^<path/.test(inner) || inner.includes("<svg")) throw new Error(`${name}: odd markup`);
  out.push([key, inner.replace(/></g, ">\n      <")]);
}
for (const [k, d] of out) console.log(`    ${k}: ${JSON.stringify(d)},`);
