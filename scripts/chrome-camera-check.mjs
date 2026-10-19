// Opens the virtual camera in headless Chrome exactly as a meeting page would (getUserMedia by
// device label) and reports what Chrome receives: resolution, delivered frame rate, and snapshots.
// Usage: node scripts/chrome-camera-check.mjs [seconds] [outdir]
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const seconds = Number(process.argv[2] ?? 8);
const outdir = process.argv[3] ?? "out/chrome-check";
mkdirSync(outdir, { recursive: true });

const page = `<!doctype html><video id=v autoplay muted playsinline></video><canvas id=c width=640 height=360></canvas>
<script>
window.run = async (seconds) => {
  const warm = await navigator.mediaDevices.getUserMedia({ video: true });
  warm.getTracks().forEach(t => t.stop());
  const cams = (await navigator.mediaDevices.enumerateDevices()).filter(d => d.kind === "videoinput");
  const cam = cams.find(d => d.label.includes("Digital Standin"));
  if (!cam) return { error: "Digital Standin not listed", cameras: cams.map(d => d.label) };
  const stream = await navigator.mediaDevices.getUserMedia({ video: { deviceId: { exact: cam.deviceId }, width: 1280, height: 720 } });
  const v = document.getElementById("v");
  v.srcObject = stream;
  await new Promise(r => v.onloadedmetadata = r);
  await v.play();
  const ctx = document.getElementById("c").getContext("2d", { willReadFrequently: true });
  let frames = 0, last = null, changes = [];
  const shots = [];
  const t0 = performance.now();
  await new Promise(done => {
    const tick = () => {
      frames++;
      ctx.drawImage(v, 0, 0, 640, 360);
      const px = ctx.getImageData(0, 0, 640, 360).data;
      let sum = 0;
      for (let i = 0; i < px.length; i += 64) sum += px[i] + px[i + 1] + px[i + 2];
      if (last !== null) changes.push(Math.abs(sum - last));
      last = sum;
      const el = performance.now() - t0;
      if (shots.length < Math.floor(el / 1000) + 1 && shots.length < seconds) shots.push(document.getElementById("c").toDataURL("image/jpeg", 0.8));
      if (el < seconds * 1000) v.requestVideoFrameCallback(tick); else done();
    };
    v.requestVideoFrameCallback(tick);
  });
  const s = stream.getVideoTracks()[0].getSettings();
  stream.getTracks().forEach(t => t.stop());
  return { label: cam.label, cameras: cams.map(d => d.label), width: v.videoWidth, height: v.videoHeight,
    settings: { frameRate: s.frameRate, width: s.width, height: s.height },
    fps: frames / seconds, framesWithChange: changes.filter(c => c > 0).length, frames, shots };
};
</script>`;

const server = createServer((_, res) => { res.setHeader("content-type", "text/html"); res.end(page); });
await new Promise(r => server.listen(0, "127.0.0.1", r));
const url = `http://127.0.0.1:${server.address().port}/`;

const profile = mkdtempSync(join(tmpdir(), "standin-chrome-"));
const port = 9300 + Math.floor(Math.random() * 500);
const chrome = spawn("google-chrome", [
  "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
  "--use-fake-ui-for-media-stream", "--no-first-run", "--no-default-browser-check", url,
], { stdio: "ignore" });

const sleep = ms => new Promise(r => setTimeout(r, ms));
let target;
for (let i = 0; i < 50 && !target; i++) {
  await sleep(200);
  try { target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(t => t.type === "page" && t.url === url); } catch {}
}
if (!target) throw new Error("Chrome did not start");
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise(r => ws.onopen = r);
let id = 0;
const pending = new Map();
ws.onmessage = e => { const m = JSON.parse(e.data); pending.get(m.id)?.(m); };
const send = (method, params = {}) => new Promise(r => { pending.set(++id, r); ws.send(JSON.stringify({ id, method, params })); });

await sleep(500);
const reply = await send("Runtime.evaluate", { expression: `run(${seconds})`, awaitPromise: true, returnByValue: true });
const result = reply.result?.result?.value ?? { error: JSON.stringify(reply.result?.exceptionDetails ?? reply) };
(result.shots ?? []).forEach((d, i) => writeFileSync(join(outdir, `chrome-${i}.jpg`), Buffer.from(d.split(",")[1], "base64")));
delete result.shots;
console.log(JSON.stringify(result, null, 1));
ws.close();
chrome.kill();
server.close();
await sleep(300);
rmSync(profile, { recursive: true, force: true });
process.exit(result.error ? 1 : 0);
