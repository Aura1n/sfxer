// Runs a JavaScript expression inside the webview of an already-running app and
// prints the returned value. WebView2 exposes Chrome DevTools Protocol when the
// process starts with a debugging port, which makes the shipped UI inspectable
// without a dev server:
//
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 \
//     src-tauri/target/release/sfx-manager.exe &
//   node tools/ui-probe.mjs "document.querySelectorAll('.row').length"
//   node tools/ui-probe.mjs --shot shot.png
import { setTimeout as sleep } from "node:timers/promises";

const port = process.env.UI_PROBE_PORT || "9222";
// WebView2 binds the debug port to whichever loopback the OS resolves first, so
// a hard-coded 127.0.0.1 intermittently refuses while [::1] is listening.
const hosts = ["localhost", "127.0.0.1"];
const expression = process.argv[2];

if (!expression) {
  console.error("usage: node tools/ui-probe.mjs \"<js expression>\"");
  process.exit(2);
}

let targets = null;
for (const host of hosts) {
  try {
    targets = await fetch(`http://${host}:${port}/json/list`).then((r) => r.json());
    break;
  } catch (_) {}
}
if (!targets) {
  console.error(`no CDP endpoint on port ${port}; is the app running with the debug port?`);
  process.exit(1);
}
const page = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
if (!page) {
  console.error(`no inspectable page on port ${port}; is the app running with the debug port?`);
  process.exit(1);
}

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = () => reject(new Error("websocket connection failed"));
});

let id = 0;
const pending = new Map();
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
};

function send(method, params) {
  const n = ++id;
  const reply = new Promise((resolve) => pending.set(n, resolve));
  ws.send(JSON.stringify({ id: n, method, params }));
  return reply;
}

async function evaluate(js) {
  const msg = await send("Runtime.evaluate", {
    expression: js,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  const res = msg.result?.result;
  if (msg.exceptionDetails) {
    const text = msg.exceptionDetails.exception?.description || msg.exceptionDetails.text;
    throw new Error(text);
  }
  return res?.value;
}

// `node tools/ui-probe.mjs --shot out.png` writes what the webview actually paints.
if (expression === "--shot") {
  const file = process.argv[3] || "sfx-shot.png";
  await send("Page.enable", {});
  const shot = await send("Page.captureScreenshot", { format: "png" });
  const { writeFile } = await import("node:fs/promises");
  await writeFile(file, Buffer.from(shot.result.data, "base64"));
  console.log(`wrote ${file} (${Math.round((shot.result.data.length * 3) / 4)} bytes)`);
} else {
  const value = await evaluate(expression);
  console.log(value === undefined ? "undefined" : JSON.stringify(value, null, 1));
}

ws.close();
await sleep(50);
