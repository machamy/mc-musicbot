const assert = require('node:assert/strict');
const fs = require('node:fs');
const { chromium } = require('playwright-core');

// 서버 인가·Range는 Rust 통합 테스트가 검증한다. 여기서는 실제 포털 코드와 브라우저
// 디코더를 쓰되, 외부 영상과 곡 일정은 고정해서 네트워크 상태가 결과를 바꾸지 않게 한다.
(async () => {
  const base = process.env.PLAN05_BASE || 'http://127.0.0.1:8791';
  assert.match(base, /^http:\/\/127\.0\.0\.1:\d+$/);
  const tone = fs.readFileSync(process.env.PLAN05_AUDIO || '.devrun/plan05-tone.opus');
  const browser = await chromium.launch({ executablePath: process.env.PLAN05_CHROME, headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, serviceWorkers: 'block' });
    const errors = [];
    const downloads = [];
    const ranges = [];
    const originDownloads = [];
    const originRanges = [];
    const defaultRoutes = ['origin', 'server', 'embed'].map(source => ({ source, enabled: true }));
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.__mediaActions = {};
      const setAction = navigator.mediaSession.setActionHandler.bind(navigator.mediaSession);
      navigator.mediaSession.setActionHandler = (name, handler) => { window.__mediaActions[name] = handler; setAction(name, handler); };
      const play = HTMLMediaElement.prototype.play;
      HTMLMediaElement.prototype.play = function () {
        if (window.__rejectOnce && this.src.startsWith('blob:')) {
          window.__rejectOnce = false;
          return Promise.reject(new DOMException('gesture required', 'NotAllowedError'));
        }
        return play.call(this);
      };
      window.__fallbackPlaying = false;
      window.YT = { Player: class {
        constructor(host, options) { setTimeout(() => options.events.onReady(), 0); }
        setVolume() {} loadModule() {} unloadModule() {} setSize() {} unMute() {}
        loadVideoById() { window.__fallbackPlaying = true; }
        playVideo() { window.__fallbackPlaying = true; }
        pauseVideo() { window.__fallbackPlaying = false; }
        stopVideo() { window.__fallbackPlaying = false; }
        seekTo() {} getCurrentTime() { return 0; } getPlayerState() { return 1; }
      } };
    });
    await page.routeWebSocket('**/music/**', socket => socket.close());
    await page.route('**/music/api/guilds/1/state/hot', async route => {
      const response = await route.fetch();
      const data = await response.json();
      const track = { provider: 'YouTube', contentId: 'test-current', sourceUrl: 'https://example.com/audio', title: '직접 재생 검증', durationSeconds: 30 };
      data.current = { id: 'test-current', track, durationSeconds: 30 };
      data.next = { source: 'queue', item: { id: 'test-next', track: { ...track, contentId: 'test-next', title: '미리 받은 다음 곡' }, durationSeconds: 30 } };
      data.player = { ...data.player, isPaused: false, stopped: false, voiceConnected: true, voiceChannelId: '1', botOnline: true };
      data.positionSeconds = 0;
      data.sampledAtUtc = new Date().toISOString();
      data.startedUtc = data.sampledAtUtc;
      data.nextStartUtc = new Date(Date.now() + 30000).toISOString();
      const describe = id => ({ id, ready: true, sizeBytes: tone.length, durationSeconds: 30, streamUrl: `/music/api/guilds/1/stream/${id}`, sourceUrl: `/music/api/guilds/1/stream/${id}/source` });
      data.stream = { enabled: true, routes: defaultRoutes, prefetch: true, blobLimitBytes: 33554432, current: describe('test-current'), next: describe('test-next') };
      await route.fulfill({ response, json: data });
    });
    await page.route('**/music/api/guilds/1/web-listening', route => route.fulfill({ json: { ok: true } }));
    await page.route('**/music/api/guilds/1/stream/*', async route => {
      downloads.push(route.request().url().split('/').pop());
      if (route.request().url().includes('test-unavailable')) { await route.fulfill({ status: 503, body: 'unavailable' }); return; }
      const range = route.request().headers().range;
      if (range) {
        ranges.push(range);
        const first = Number(/^bytes=(\d+)-/.exec(range)?.[1] || 0);
        const last = Math.min(tone.length - 1, first + 65535);
        await route.fulfill({ status: 206, contentType: 'audio/ogg; codecs=opus',
          headers: { 'Accept-Ranges': 'bytes', 'Content-Range': `bytes ${first}-${last}/${tone.length}` }, body: tone.subarray(first, last + 1) });
        return;
      }
      await new Promise(resolve => setTimeout(resolve, 700));
      await route.fulfill({ status: 200, contentType: 'audio/ogg; codecs=opus', body: tone });
    });
    await page.route('**/music/api/guilds/1/stream/*/source', route => {
      const id = route.request().url().split('/').at(-2);
      return route.fulfill({ json: { source: { url: `https://test.googlevideo.com/${id}`, mime: 'audio/ogg', sizeBytes: id === 'test-origin-large' ? 33554433 : tone.length, durationSeconds: 30 } } });
    });
    await page.route('https://test.googlevideo.com/**', async route => {
      originDownloads.push(route.request().url().split('/').pop());
      if (route.request().url().includes('test-origin-fails')) { await route.fulfill({ status: 403, body: 'forbidden' }); return; }
      const range = route.request().headers().range;
      if (range) {
        originRanges.push(range);
        const first = Number(/^bytes=(\d+)-/.exec(range)?.[1] || 0);
        const last = Math.min(tone.length - 1, first + 65535);
        await route.fulfill({ status: 206, contentType: 'audio/ogg', headers: { 'Access-Control-Allow-Origin': '*',
          'Accept-Ranges': 'bytes', 'Content-Range': `bytes ${first}-${last}/${tone.length}` }, body: tone.subarray(first, last + 1) });
        return;
      }
      await new Promise(resolve => setTimeout(resolve, 700));
      await route.fulfill({ contentType: 'audio/ogg', headers: { 'Access-Control-Allow-Origin': '*' }, body: tone });
    });
    await page.goto(`${base}/music`);
    await page.getByRole('button', { name: '로컬 검증 계정으로 입장' }).click();
    await page.waitForURL('**/music/guilds/1');
    await page.waitForTimeout(1200);
    for (let attempt = 0; attempt < 4; attempt++) {
      if (!(await page.locator('.sheet-back').count())) break;
      await page.keyboard.press('Escape');
      await page.waitForTimeout(250);
    }
    await page.getByRole('button', { name: '🔊 웹에서 듣기', exact: true }).click();
    await page.waitForFunction(() => window.__fallbackPlaying);
    await page.getByRole('button', { name: '🔊 직접 받기', exact: true }).click();
    assert.equal(await page.evaluate(() => window.__fallbackPlaying), false);
    await page.waitForFunction(() => [...document.querySelectorAll('audio')].some(audio => !audio.paused && audio.currentTime > 1));
    assert.equal(await page.evaluate(() => window.__fallbackPlaying), false);
    await page.waitForFunction(() => [...document.querySelectorAll('audio')].filter(audio => audio.readyState >= 2).length === 2);
    assert.deepEqual(originDownloads, ['test-current', 'test-next']);
    assert.deepEqual(downloads, [], 'Server audio must not be fetched when origin works');
    await page.getByText('다운로드 · 이 기기에 2곡 보관 중', { exact: true }).waitFor();
    assert.equal(await page.locator('.direct-status__item').count(), 2);
    assert.match(await page.locator('.direct-status').innerText(), /서버 캐시: 준비됨/);
    await page.evaluate(() => window.__mediaActions.pause());
    await page.waitForTimeout(1700);
    assert.equal(await page.evaluate(() => [...document.querySelectorAll('audio')].every(audio => audio.paused)), true);
    await page.evaluate(() => window.__mediaActions.play());
    await page.waitForFunction(() => [...document.querySelectorAll('audio')].some(audio => !audio.paused));
    const firstBlob = await page.evaluate(() => [...document.querySelectorAll('audio')].find(audio => !audio.paused).src);
    await page.evaluate(async () => {
      const { store, clock } = await import('/music/assets/core.js');
      const state = store.get();
      clock.sync({ positionSeconds: 0, sampledAtUtc: new Date().toISOString(), startedUtc: new Date().toISOString(), durationSeconds: 30, isPaused: false, stopped: false });
      store.patch({ current: state.next.item, stream: { ...state.stream, current: state.stream.next, next: null } });
    });
    await page.waitForFunction(() => [...document.querySelectorAll('audio')].filter(audio => !audio.paused).length === 1);
    assert.notEqual(await page.evaluate(() => [...document.querySelectorAll('audio')].find(audio => !audio.paused).src), firstBlob);
    assert.equal(originDownloads.length, 2);
    await page.screenshot({ path: '.devrun/plan05-direct.png' });
    await page.getByRole('button', { name: '🔊 직접 받기 켜짐', exact: true }).click();
    await page.waitForFunction(() => window.__fallbackPlaying && [...document.querySelectorAll('audio')].every(audio => audio.paused && !audio.getAttribute('src')));
    await page.evaluate(() => { window.__rejectOnce = true; });
    await page.getByRole('button', { name: '🔊 직접 받기', exact: true }).click();
    await page.getByRole('button', { name: '직접 재생', exact: true }).waitFor({ state: 'visible' });
    assert.equal(await page.evaluate(() => window.__fallbackPlaying), false);
    await page.getByRole('button', { name: '직접 재생', exact: true }).click();
    await page.waitForFunction(() => [...document.querySelectorAll('audio')].some(audio => !audio.paused));
    await page.evaluate(async () => {
      const { store } = await import('/music/assets/core.js');
      const current = { ...store.get().current, id: 'test-large' };
      store.patch({ current, stream: { ...store.get().stream, current: { id: current.id, ready: true,
        sizeBytes: 33554433, durationSeconds: 30, streamUrl: '/music/api/guilds/1/stream/test-large' }, next: null } });
    });
    await page.waitForFunction(() => [...document.querySelectorAll('audio')].some(audio => audio.currentSrc.includes('/stream/test-large') && !audio.paused && audio.currentTime > 1));
    assert.ok(ranges.length > 0, 'Native audio must request byte ranges');
    assert.equal(await page.evaluate(() => window.__fallbackPlaying), false);
    assert.match(await page.locator('.direct-status').innerText(), /구간 재생 \(Range\)/);
    await page.getByText('다운로드 · 이 기기에 0곡 보관 중', { exact: true }).waitFor();
    assert.match(await page.locator('.webnote').innerText(), /네트워크 연결이 필요/);
    const serverDownloadsBeforeOrigin = downloads.length;
    await page.evaluate(async () => {
      const { store } = await import('/music/assets/core.js');
      const current = { ...store.get().current, id: 'test-origin-large' };
      store.patch({ current, stream: { ...store.get().stream, current: { id: current.id, ready: true,
        sizeBytes: 33554433, durationSeconds: 30, streamUrl: '/music/api/guilds/1/stream/test-origin-large', sourceUrl: '/music/api/guilds/1/stream/test-origin-large/source' }, next: null } });
    });
    await page.waitForFunction(() => [...document.querySelectorAll('audio')].some(audio => audio.currentSrc.includes('test.googlevideo.com/test-origin-large') && !audio.paused));
    assert.ok(originRanges.length > 0);
    assert.equal(downloads.length, serverDownloadsBeforeOrigin);
    await page.evaluate(async () => {
      const { store } = await import('/music/assets/core.js');
      const current = { ...store.get().current, id: 'test-origin-fails' };
      store.patch({ current, stream: { ...store.get().stream, current: { id: current.id, ready: true,
        sizeBytes: 500000, durationSeconds: 30, streamUrl: '/music/api/guilds/1/stream/test-origin-fails', sourceUrl: '/music/api/guilds/1/stream/test-origin-fails/source' }, next: null } });
    });
    await page.waitForFunction(() => [...document.querySelectorAll('audio')].some(audio => audio.currentSrc.startsWith('blob:') && !audio.paused));
    assert.ok(originDownloads.includes('test-origin-fails'));
    assert.ok(downloads.includes('test-origin-fails'));
    assert.match(await page.locator('.direct-status').innerText(), /서버 캐시 → 이 기기/);
    await page.evaluate(async () => {
      const { store } = await import('/music/assets/core.js');
      const current = { ...store.get().current, id: 'test-unavailable' };
      store.patch({ current, stream: { ...store.get().stream, current: { id: current.id, ready: true,
        sizeBytes: 500000, durationSeconds: 30, streamUrl: '/music/api/guilds/1/stream/test-unavailable' }, next: null } });
    });
    await page.waitForFunction(() => window.__fallbackPlaying);
    await page.evaluate(async () => {
      const { store } = await import('/music/assets/core.js');
      store.patch({ stream: { ...store.get().stream, routes: [] } });
    });
    await page.waitForFunction(() => !window.__fallbackPlaying && [...document.querySelectorAll('audio')].every(audio => audio.paused));
    const attemptsBeforeEmbed = downloads.length + originDownloads.length;
    await page.evaluate(async () => {
      const { store } = await import('/music/assets/core.js');
      store.patch({ stream: { ...store.get().stream, routes: ['embed', 'server', 'origin'].map(source => ({ source, enabled: true })) } });
    });
    await page.waitForFunction(() => window.__fallbackPlaying);
    assert.equal(downloads.length + originDownloads.length, attemptsBeforeEmbed);
    await page.evaluate(async () => {
      const { store } = await import('/music/assets/core.js');
      store.patch({ stream: { ...store.get().stream, enabled: false } });
    });
    await page.waitForFunction(() => window.__fallbackPlaying && [...document.querySelectorAll('audio')].every(audio => audio.paused && !audio.getAttribute('src')));
    await page.getByRole('button', { name: '🔊 웹에서 듣는 중', exact: true }).click();
    assert.equal(await page.evaluate(() => window.__fallbackPlaying), false);
    let savedSettings = null;
    await page.route('**/music/guilds/1/admin**', async route => {
      const response = await route.fetch();
      const html = (await response.text()).replace(/"tier"\s*:\s*"manager"/g, '"tier":"owner"');
      await route.fulfill({ response, body: html });
    });
    await page.route('**/music/api/owner/stream', async route => {
      if (route.request().method() === 'PUT') savedSettings = route.request().postDataJSON();
      await route.fulfill({ json: { enabled: false, maxTransfers: 10, bandwidthKbps: 2000, active: 0, queued: 0, ...savedSettings } });
    });
    let playbackPatch = null;
    await page.route('**/music/api/owner/playback', async route => {
      if (route.request().method() === 'PUT') playbackPatch = route.request().postDataJSON();
      await route.fulfill({ json: { masterVolume: 100, normalizeEnabled: true, autoplayDefault: true, announceNowPlaying: true,
        emptyVoiceForced: false, autoLeaveWhenEmpty: true, autoLeaveDelaySeconds: 60, emptyVoicePolicy: 'AutoLeave',
        cacheLimitGb: 30, logRetentionDays: 14, sponsorblockRemove: false, tweakFfmpegFastStart: false,
        tweakFfmpegDirectOutput: false, voiceBitrateKbps: 128, ...playbackPatch } });
    });
    await page.goto(`${base}/music/owner`);
    await page.getByRole('heading', { name: '웹 직접 받기', exact: true }).waitFor();
    await page.getByLabel('웹 직접 받기 허용').check();
    await page.getByLabel('전체 업로드 상한 (128~20,000 kbps)').fill('16000');
    await page.getByRole('button', { name: '봇 서버 캐시에서 받기 우선순위 올리기', exact: true }).click();
    await page.getByLabel('다음 곡도 미리 받기', { exact: true }).uncheck();
    await page.getByRole('button', { name: '직접 받기 설정 저장' }).click();
    await page.waitForTimeout(300);
    assert.deepEqual(savedSettings, { enabled: true, maxTransfers: 10, bandwidthKbps: 16000, prefetch: false,
      routes: [defaultRoutes[1], defaultRoutes[0], defaultRoutes[2]] });
    await page.getByLabel('마스터 볼륨 (0~200)', { exact: true }).fill('123');
    await page.getByRole('button', { name: '전역 재생 기본값 저장', exact: true }).click();
    await page.waitForTimeout(300);
    assert.deepEqual(playbackPatch, { masterVolume: 123 });
    const playbackPanel = page.getByRole('heading', { name: '전역 재생 기본값·호스트 자원', exact: true }).locator('..');
    for (const width of [1440, 412]) {
      await page.setViewportSize({ width, height: 1000 });
      assert.equal(await playbackPanel.evaluate(panel => [...panel.querySelectorAll('label')].every(label => {
        const bounds = label.getBoundingClientRect();
        const control = label.querySelector('input, select').getBoundingClientRect();
        return control.top >= bounds.top && control.bottom <= bounds.bottom + 1 && control.right <= bounds.right + 1;
      })), true, `owner controls fit their labels at ${width}px`);
      await playbackPanel.screenshot({ path: `.devrun/plan05-console-${width}.png` });
    }
    assert.deepEqual(errors, []);
    console.log('PASS: 원본 Blob/Range 우선 → 원본 실패 시 서버 → 서버 실패 시 임베드, 현재/다음 미리받기, 순서·비활성 정책, 진행·보관 현황, OS pause/play, 단일 봇 주인 패널 저장, JS 오류 0');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
