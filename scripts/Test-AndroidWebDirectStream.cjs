const fs = require('node:fs');
const assert = require('node:assert/strict');
const http = require('node:http');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { ws: WebSocket, wsServer: WebSocketServer } = require('playwright-core/lib/utilsBundle');
const sdk = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT || path.join(process.env.LOCALAPPDATA, 'Android/Sdk');
const adbPath = path.join(sdk, 'platform-tools/adb.exe');
const serial = process.env.PLAN05_DEVICE || 'emulator-5554';
assert.match(serial, /^emulator-\d+$/, 'This test only operates on an emulator');
const adb = (...args) => execFileSync(adbPath, ['-s', serial, ...args], { encoding: 'utf8' }).trim();
const pause = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
const rangeMode = process.env.PLAN05_RANGE === '1';
const output = `.devrun/plan05-avd-${rangeMode ? 'range' : 'blob'}`;
const tone = fs.readFileSync(rangeMode ? '.devrun/plan05-avd-long.opus' : '.devrun/plan05-avd-tone.opus');
assert.ok(rangeMode ? tone.length > 33554432 : tone.length <= 33554432, 'Fixture size must select the intended playback mode');
const reports = [];
const results = [];
const requests = [];
let start = Date.now();
let scenario = '';
const duration = rangeMode ? 2100 : 90;
function playback() {
  const elapsed = (Date.now() - start) / 1000;
  const index = Math.floor(elapsed / duration);
  const item = offset => ({ id: `avd-${index + offset}`, durationSeconds: duration,
    track: { provider: 'YouTube', contentId: `avd-${index + offset}`, sourceUrl: 'https://example.com/audio', title: `AVD track ${index + offset}`, durationSeconds: duration } });
  const describe = offset => ({ id: item(offset).id, ready: true, sizeBytes: tone.length, durationSeconds: duration,
    streamUrl: `/music/api/guilds/1/stream/${item(offset).id}`, sourceUrl: `/music/api/guilds/1/stream/${item(offset).id}/source` });
  return { current: item(0), currentId: item(0).id, next: { source: 'queue', item: item(1) }, durationSeconds: duration,
    isPaused: false, stopped: false, voiceConnected: true, botOnline: true, voiceChannelId: '1',
    positionSeconds: elapsed % duration, sampledAtUtc: new Date().toISOString(), startedUtc: new Date(start + index * duration * 1000).toISOString(),
    nextStartUtc: new Date(start + (index + 1) * duration * 1000).toISOString(),
    stream: { enabled: true, prefetch: true, routes: ['origin', 'server', 'embed'].map(source => ({ source, enabled: true })),
      blobLimitBytes: 33554432, current: describe(0), next: describe(1) } };
}
const instrumentation = `<script>
window.YT={Player:class{constructor(host,options){setTimeout(()=>options.events.onReady(),0)}setVolume(){}loadModule(){}unloadModule(){}setSize(){}unMute(){}loadVideoById(){}playVideo(){}pauseVideo(){}stopVideo(){}seekTo(){}getCurrentTime(){return 0}getPlayerState(){return 1}}};
localStorage.setItem('macham.direct-stream','0');
function report(event){navigator.sendBeacon('/test/report',JSON.stringify({event,wall:Date.now(),hidden:document.hidden,audio:[...document.querySelectorAll('audio')].map(audio=>({time:audio.currentTime,paused:audio.paused,ended:audio.ended,src:audio.currentSrc,duration:audio.duration})),title:navigator.mediaSession?.metadata?.title}));}
document.addEventListener('visibilitychange',()=>report('visibility'),true);
for(const event of ['playing','pause','ended','error'])document.addEventListener(event,change=>{if(change.target.tagName==='AUDIO')report(event)},true);
window.addEventListener('error',error=>navigator.sendBeacon('/test/report',JSON.stringify({event:'js-error',message:error.message})));
setInterval(()=>report('beat'),2000);
</script>`;
const server = http.createServer(async (request, response) => {
  try {
    if (request.url === '/test/report') {
      let body = ''; for await (const part of request) body += part;
      const report = { scenario, ...JSON.parse(body) }; reports.push(report);
      fs.appendFileSync(`${output}.jsonl`, JSON.stringify(report) + '\n');
      response.writeHead(204).end(); return;
    }
    if (request.url.startsWith('/music/api/guilds/1/stream/') && request.url.endsWith('/source')) {
      response.writeHead(200, { 'Content-Type': 'application/json' }).end('{"source":null}'); return;
    }
    if (request.url.startsWith('/music/api/guilds/1/stream/')) {
      const match = /^bytes=(\d+)-(\d*)$/.exec(request.headers.range || '');
      const first = match ? Number(match[1]) : 0;
      const last = match ? Math.min(tone.length - 1, first + 1048575, match[2] ? Number(match[2]) : Infinity) : tone.length - 1;
      const headers = { 'Content-Type': 'audio/ogg; codecs=opus', 'Content-Length': last - first + 1, 'Cache-Control': 'no-store', 'Accept-Ranges': 'bytes' };
      if (match) headers['Content-Range'] = `bytes ${first}-${last}/${tone.length}`;
      console.log('AUDIO_REQUEST', JSON.stringify({ scenario, range: request.headers.range, first, last, at: (Date.now() - start) / 1000 }));
      requests.push({ scenario, range: request.headers.range });
      response.writeHead(match ? 206 : 200, headers);
      for (let offset = first; offset <= last && !response.destroyed; offset += 16384) { response.write(tone.subarray(offset, Math.min(last + 1, offset + 16384))); await pause(15); }
      response.end(); return;
    }
    if (request.url.endsWith('/web-listening')) { response.writeHead(200, { 'Content-Type': 'application/json' }); response.end('{"ok":true}'); return; }
    const headers = { ...request.headers }; delete headers.host; delete headers['accept-encoding'];
    const parts = []; for await (const part of request) parts.push(part);
    const upstream = await fetch(`http://127.0.0.1:8791${request.url}`, { method: request.method, headers, body: parts.length ? Buffer.concat(parts) : undefined, redirect: 'manual' });
    let bytes = Buffer.from(await upstream.arrayBuffer());
    if (request.url.includes('/state/hot')) {
      const data = JSON.parse(bytes); const state = playback();
      Object.assign(data, state); data.player = { ...data.player, ...state };
      bytes = Buffer.from(JSON.stringify(data));
    } else if ((upstream.headers.get('content-type') || '').includes('text/html')) {
      bytes = Buffer.from(bytes.toString().replace('<head>', '<head>' + instrumentation));
    }
    const outgoing = Object.fromEntries(upstream.headers); delete outgoing['content-encoding']; delete outgoing['transfer-encoding'];
    outgoing['content-length'] = bytes.length; outgoing['cache-control'] = 'no-store';
    response.writeHead(upstream.status, outgoing); response.end(bytes);
  } catch (error) { response.writeHead(500).end(String(error)); }
});
const sockets = new WebSocketServer({ server });
const broadcast = setInterval(() => { for (const socket of sockets.clients) if (socket.readyState === 1) socket.send(JSON.stringify({ t: 'playback', d: playback() })); }, 1000);
async function connect() {
  const pages = await (await fetch('http://127.0.0.1:9222/json/list')).json();
  const target = pages.find(page => page.type === 'page' && page.url.includes('localhost:8793'));
  if (!target) throw Error('AVD portal target missing');
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise(resolve => socket.once('open', resolve));
  let serial = 0; const pending = new Map();
  socket.on('message', raw => { const message = JSON.parse(raw); if (message.id && pending.has(message.id)) { pending.get(message.id)(message); pending.delete(message.id); } });
  const send = (method, params) => new Promise((resolve, reject) => { const id = ++serial; const timeout = setTimeout(() => reject(Error('CDP timeout')), 15000); pending.set(id, message => { clearTimeout(timeout); message.error ? reject(Error(JSON.stringify(message.error))) : resolve(message.result); }); socket.send(JSON.stringify({ id, method, params })); });
  const evaluate = async expression => { const value = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (value.exceptionDetails) throw Error(JSON.stringify(value.exceptionDetails)); return value.result.value; };
  const click = async text => {
    const rectangle = await evaluate(`(()=>{const button=[...document.querySelectorAll('button')].find(button=>button.textContent.trim()===${JSON.stringify(text)}&&!button.hidden);if(!button)return null;button.scrollIntoView({block:'center'});const rect=button.getBoundingClientRect();return {x:rect.x+rect.width/2,y:rect.y+rect.height/2}})()`);
    if (!rectangle) throw Error('Button not found: ' + text);
    await send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ ...rectangle }] });
    await send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  };
  return { socket, evaluate, click, send };
}
async function run(name, seconds, lock) {
  scenario = name; start = Date.now() - (rangeMode ? (name === 'boundary' ? 2030 : 600) * 1000 : 0);
  adb('shell', 'input', 'keyevent', 'KEYCODE_WAKEUP'); adb('shell', 'wm', 'dismiss-keyguard');
  adb('shell', 'am', 'start', '-a', 'android.intent.action.VIEW', '-d', 'http://localhost:8793/music', '-n', 'com.android.chrome/com.google.android.apps.chrome.Main');
  await pause(4000);
  let client = await connect();
  if (await client.evaluate(`!![...document.querySelectorAll('button')].find(button=>button.textContent.includes('로컬 검증 계정으로 입장'))`)) {
    await client.click('로컬 검증 계정으로 입장'); await pause(3000);
  }
  for (let attempt = 0; attempt < 5; attempt++) {
    await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
    await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 }); await pause(200);
  }
  if (await client.evaluate(`!![...document.querySelectorAll('button')].find(button=>button.textContent.trim()==='🔊 웹에서 듣기')`)) await client.click('🔊 웹에서 듣기');
  await client.click('🔊 직접 받기');
  if (!rangeMode) {
    let progress = null;
    for (let attempt = 0; attempt < 15; attempt++) {
      await pause(100);
      progress = await client.evaluate(`(()=>{const progress=document.querySelector('.direct-status progress');return progress&&{value:progress.value,max:progress.max,text:document.querySelector('.direct-status').textContent}})()`);
      if (progress?.value > 0 && progress.value < progress.max) break;
    }
    assert.ok(progress?.value > 0 && progress.value < progress.max, 'Partial download progress must be visible');
    console.log('DOWNLOAD_PROGRESS', JSON.stringify(progress));
  }
  for (let attempt = 0; attempt < 20; attempt++) {
    await pause(1000);
    if (await client.evaluate(`!![...document.querySelectorAll('button')].find(button=>button.textContent.trim()==='직접 재생'&&!button.hidden)`)) await client.click('직접 재생');
    if (await client.evaluate(`[...document.querySelectorAll('audio')].some(audio=>!audio.paused&&audio.currentTime>1)`)) break;
    if (attempt === 19) throw Error('Direct audio did not start');
  }
  await pause(2000);
  console.log(name, 'foreground', await client.evaluate(`[...document.querySelectorAll('audio')].map(audio=>({time:audio.currentTime,paused:audio.paused,ready:audio.readyState}))`));
  client.socket.close(); await pause(1000);
  adb('shell', 'input', 'keyevent', lock ? 'KEYCODE_SLEEP' : 'KEYCODE_HOME');
  const platform = [];
  for (let elapsed = 0; elapsed < seconds; elapsed += 10) {
    await pause(Math.min(10, seconds - elapsed) * 1000);
    const dump = adb('shell', 'dumpsys', 'audio');
    platform.push((dump.match(/state:started/g) || []).length);
  }
  adb('shell', 'input', 'keyevent', 'KEYCODE_WAKEUP'); adb('shell', 'wm', 'dismiss-keyguard');
  adb('shell', 'am', 'start', '-n', 'com.android.chrome/com.google.android.apps.chrome.Main');
  await pause(3000);
  const entries = reports.filter(report => report.scenario === name);
  const hide = entries.find(report => report.event === 'visibility' && report.hidden && report.audio.some(audio => !audio.paused));
  const resume = entries.find(report => report.event === 'visibility' && !report.hidden && report.wall > hide?.wall);
  const result = { name, seconds, platform, hide, resume, hiddenPlaying: entries.filter(report => report.event === 'playing' && report.hidden), errors: entries.filter(report => report.event === 'js-error') };
  const ended = entries.find(report => report.event === 'ended' && report.hidden);
  if (ended && result.hiddenPlaying.length) result.boundaryGapMs = result.hiddenPlaying[0].wall - ended.wall;
  results.push(result); console.log('RESULT', JSON.stringify(result));
  assert.ok(hide && resume, 'Visibility snapshots must bracket background playback');
  const before = hide.audio.find(audio => !audio.paused);
  const after = resume.audio.find(audio => !audio.paused);
  assert.ok(before && after, 'Audio must be playing on both sides of the background interval');
  const crossed = hide.title !== resume.title;
  const ratio = (after.time - before.time + (crossed ? duration : 0)) / ((resume.wall - hide.wall) / 1000);
  result.ratio = ratio;
  assert.ok(ratio > 0.9 && ratio < 1.1, `Background playback ratio ${ratio}`);
  assert.ok(platform.every(count => count > 0), 'Android audio must remain started in every sample');
  assert.equal(result.errors.length, 0);
  if (name === 'boundary') assert.ok(crossed && result.hiddenPlaying.length > 0, 'Next track must start while hidden');
  if (rangeMode) assert.ok(requests.some(request => request.scenario === name && request.range), 'Native audio must issue Range requests');
  adb('shell', 'screencap', '-p', '/sdcard/plan05-avd.png'); adb('pull', '/sdcard/plan05-avd.png', `${output}-${name}.png`);
  adb('shell', 'am', 'force-stop', 'com.android.chrome');
}
(async () => {
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(8793, '127.0.0.1', resolve); });
  adb('reverse', 'tcp:8793', 'tcp:8793'); adb('forward', 'tcp:9222', 'localabstract:chrome_devtools_remote');
  console.log('ANDROID', adb('shell', 'getprop', 'ro.build.version.release'));
  console.log('CHROME', adb('shell', 'dumpsys', 'package', 'com.android.chrome').match(/versionName=.+/)[0]);
  try { await run('home', 45, false); await run('lock', 45, true); await run('boundary', 110, true); }
  finally {
    clearInterval(broadcast); for (const socket of sockets.clients) socket.terminate(); sockets.close(); server.close();
    fs.writeFileSync(`${output}-results.json`, JSON.stringify(results, null, 2));
    adb('reverse', '--remove', 'tcp:8793'); adb('forward', '--remove', 'tcp:9222');
  }
})().catch(error => {
  console.error(error); clearInterval(broadcast);
  for (const socket of sockets.clients) socket.terminate();
  sockets.close(); server.close(); process.exitCode = 1;
});
