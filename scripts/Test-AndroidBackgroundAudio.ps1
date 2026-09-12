# 안드로이드에서 배경 재생이 되는지 **로컬에서** 재는 시험대.
#
# 왜 있나 — "폰에서 홈 화면으로 나가면 소리가 멈춘다" 를 세 번 손봤는데 세 번 다 남의 폰으로만
# 확인했다(v4.55 · v4.57 · v4.69). 고쳐졌는지 알려면 매번 남에게 부탁해야 했고, 두 번은
# "고쳤다" 고 적은 뒤에 안 고쳐진 것이 드러났다. 여기서는 에뮬레이터에 실제 크롬을 띄워 직접 잰다.
#
# 두 갈래를 같은 조건으로 돌린다.
#     A  우리 오리진의 <audio>      ← PLAN-05 가 가려는 곳
#     B  유튜브 1×1 iframe          ← 지금 방식 (대조군)
#
# **대조군이 핵심이다.** B 가 멈추는 것을 같이 보지 못하면, A 가 계속 재생된 것이 "에뮬레이터가
# 원래 안 재우기 때문" 인지 구별할 수 없다. 둘을 같이 재야 결과가 말이 된다.
#
# 판정은 서로 독립한 두 갈래로 받는다.
#   1. 페이지가 1초마다 서버로 보내는 심박 — 돌아온 뒤 `currentTime` 이 벽시계만큼 흘렀는가.
#      배경에서 JS 가 얼어도 이 값은 거짓말을 못 한다.
#   2. `adb shell dumpsys audio` 의 `state:started` 개수 — JS 와 무관한 플랫폼 신호.
#      브라우저가 뭐라고 하든 안드로이드가 실제로 소리를 내고 있었는지는 여기 남는다.
#
# 쓰는 법
#     .\scripts\Test-AndroidBackgroundAudio.ps1
#     .\scripts\Test-AndroidBackgroundAudio.ps1 -Seconds 60 -KeepEmulator
#     .\scripts\Test-AndroidBackgroundAudio.ps1 -Only A
#
# **한계 — 에뮬레이터는 제조사 절전 관리가 없는 맨 안드로이드다.**
#   * 여기서 **멈추면** 실기기에서도 확실히 멈춘다 — 결론으로 써도 된다.
#   * 여기서 **계속 되면** 실기기 한 대로는 확인해야 한다. 삼성·샤오미는 더 조인다.
# 25초 홈 시험은 "즉시 멈추는가" 만 답한다. 30분 이상·화면 잠금·절전 모드는 실기기 몫이다.

[CmdletBinding()]
param(
    # 비우면 설치된 AVD 중 첫 번째. `google_apis_playstore` 이미지여야 크롬이 들어 있다.
    [string]$Avd = '',
    [int]$Port = 8777,
    # 배경에 머무는 시간. 짧으면 절전이 안 걸려 통과하기 쉽다.
    [int]$Seconds = 25,
    [ValidateSet('AB', 'A', 'B')][string]$Only = 'AB',
    # 대조군에 쓸 영상. 생방송은 getCurrentTime 이 달라서 일반 영상을 쓴다.
    [string]$VideoId = 'dQw4w9WgXcQ',
    # 끝나도 에뮬레이터를 켜 둔다. 연달아 돌릴 때 부팅 시간을 아낀다.
    [switch]$KeepEmulator
)

$ErrorActionPreference = 'Stop'

# ── SDK 찾기 ────────────────────────────────────────────────────────────────
# PATH 에 없는 게 보통이다(스튜디오가 안 넣는다). 흔한 자리를 직접 본다.
function Resolve-Sdk {
    $candidates = @($env:ANDROID_HOME, $env:ANDROID_SDK_ROOT,
        "$env:LOCALAPPDATA\Android\Sdk", "$env:USERPROFILE\AppData\Local\Android\Sdk")
    foreach ($c in $candidates) {
        if ($c -and (Test-Path (Join-Path $c 'platform-tools\adb.exe'))) { return (Resolve-Path $c).Path }
    }
    throw '안드로이드 SDK 를 못 찾았어요. ANDROID_HOME 을 설정하거나 스튜디오로 SDK 를 받으세요.'
}

$Sdk = Resolve-Sdk
$Adb = Join-Path $Sdk 'platform-tools\adb.exe'
$Emu = Join-Path $Sdk 'emulator\emulator.exe'
Write-Host "[test] SDK $Sdk"

# adb 는 진행 상황을 **stderr 로** 쓴다(`pull` 의 "1 file pulled" 가 그렇다). 이 스크립트는
# `$ErrorActionPreference = 'Stop'` 이라, 그게 NativeCommandError 로 감싸여 **성공한 명령에서
# 스크립트가 죽는다.** 실제로 그렇게 죽었다. 여기서만 Continue 로 낮춘다.
function Invoke-Adb {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $Adb @args 2>&1 } finally { $ErrorActionPreference = $saved }
}

function Wait-Boot([int]$TimeoutSec = 180) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        $ok = ''
        try { $ok = (Invoke-Adb shell getprop sys.boot_completed 2>$null) -join '' } catch { }
        if ($ok.Trim() -eq '1') { return }
        Start-Sleep -Seconds 3
    }
    throw "에뮬레이터가 $TimeoutSec 초 안에 안 떴어요."
}

# ── 에뮬레이터 ──────────────────────────────────────────────────────────────
$startedEmulator = $false
if (((Invoke-Adb devices) -join "`n") -notmatch 'emulator-\d+\s+device') {
    if (-not $Avd) {
        $list = @(& $Emu -list-avds 2>$null | Where-Object { $_ -and $_.Trim() })
        if (-not $list.Count) { throw 'AVD 가 없어요. 크롬이 필요하니 Play Store 이미지로 하나 만드세요.' }
        $Avd = $list[0].Trim()
    }
    Write-Host "[test] 에뮬레이터 기동: $Avd"
    # **-no-audio 를 넣으면 안 된다** — 오디오 시험인데 오디오 장치를 빼면 결과가 거짓이 된다.
    Start-Process -FilePath $Emu -WindowStyle Hidden -ArgumentList @(
        '-avd', $Avd, '-no-window', '-no-boot-anim', '-gpu', 'swiftshader_indirect')
    $startedEmulator = $true
    Wait-Boot
} else {
    Write-Host '[test] 이미 켜진 에뮬레이터를 씁니다.'
    Wait-Boot 30
}

if (((Invoke-Adb shell pm list packages com.android.chrome) -join '') -notmatch 'com\.android\.chrome') {
    throw '이 AVD 에 크롬이 없어요. `google_apis_playstore` 시스템 이미지로 만든 AVD 가 필요합니다.'
}

# ── 시험 자산 ───────────────────────────────────────────────────────────────
$Root = Join-Path ([IO.Path]::GetTempPath()) ('mc-bgtest-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $Root -Force | Out-Null
$LogPath = Join-Path $Root 'report.log'
New-Item -ItemType File -Path $LogPath -Force | Out-Null

if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    throw 'ffmpeg 이 PATH 에 없어요. 시험용 오디오를 만들 수 없습니다.'
}
# 무음이면 오디오 포커스가 안 잡힐 수 있어 톤을 넣는다. 128k opus — 봇 캐시와 같은 형식.
#
# **길이를 필요한 만큼만 만든다.** 처음엔 900초(16.5MB)를 썼는데, 아래 시험 서버가 단일
# 스레드라 그 파일을 내려보내는 9초 동안 **보고 POST 가 전부 뒤에 줄을 섰다.** 그래서
# 스크립트는 "재생이 안 시작됐다" 고 판단하고 포기했는데, 실제로는 재생 중이었다.
# 측정 창(`$Seconds`)보다 넉넉히 길기만 하면 되므로 그만큼만 만든다.
$clipSec = $Seconds * 3 + 180
& ffmpeg -y -loglevel error -f lavfi -i "sine=frequency=440:duration=$clipSec" `
    -c:a libopus -b:a 128k (Join-Path $Root 'test.opus')
if ($LASTEXITCODE -ne 0) { throw 'ffmpeg 이 시험용 opus 를 못 만들었어요.' }
$clipMb = [Math]::Round((Get-Item (Join-Path $Root 'test.opus')).Length / 1MB, 1)
Write-Host "[test] 시험 음원 ${clipSec}초 · ${clipMb}MB"

# 페이지 공통. **화면 아무 곳이나 눌러도 시작한다** — 버튼 좌표를 맞히려 들면 화면 크기가
# 다른 기기에서 곧바로 깨진다. 가운데를 한 번 누르면 되게 해서 그 문제를 없앤다.
$shell = @'
<!doctype html><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>html,body{margin:0;height:100%}body{font:16px system-ui;background:#111;color:#eee;
display:flex;flex-direction:column;justify-content:center;align-items:center;text-align:center;padding:16px}
h2{margin:0 0 8px}p{color:#9aa}pre{font-size:11px;white-space:pre-wrap}</style>
'@

$measure = @'
function post(o){ o.kind=KIND; var b=JSON.stringify(o);
  try{ if(navigator.sendBeacon){ navigator.sendBeacon('/report', new Blob([b],{type:'application/json'})); return; } }catch(e){}
  fetch('/report',{method:'POST',body:b,keepalive:true}).catch(function(){}); }
function show(m){ var p=document.getElementById('out'); if(p) p.textContent=m+'\n'+p.textContent; }
var lastHidden=null, beating=false;
function beginBeat(readTime, extra){
  if(beating) return; beating=true;
  post({ev:'start', t:readTime()});
  if('mediaSession' in navigator){
    try{ navigator.mediaSession.metadata=new MediaMetadata({title:'배경 시험',artist:'마참뮤직'});
         navigator.mediaSession.playbackState='playing'; }catch(e){}
  }
  setInterval(function(){
    var o={ev:'beat', t:+readTime().toFixed(2), wall:Date.now(), hidden:document.hidden};
    if(extra){ extra(o); }
    post(o);
  }, 1000);
  document.addEventListener('visibilitychange', function(){
    if(document.hidden){ lastHidden={t:readTime(), wall:Date.now()};
      post({ev:'hide', t:+readTime().toFixed(2)}); return; }
    if(!lastHidden){ return; }
    /* **핵심 측정.** 배경에서 JS 가 얼어도, 돌아온 뒤 currentTime 이 벽시계만큼 흘렀는지
       보면 그 사이 재생됐는지 알 수 있다. 심박이 끊긴 경우에도 이 한 줄이 답을 준다. */
    var wall=(Date.now()-lastHidden.wall)/1000, adv=readTime()-lastHidden.t;
    var ratio = wall>0 ? adv/wall : 0;
    var verdict = ratio>0.7 ? 'PLAYED' : (ratio<0.2 ? 'PAUSED' : 'PARTIAL');
    show('복귀: 벽시계 '+wall.toFixed(1)+'s / 진행 '+adv.toFixed(1)+'s → '+verdict);
    post({ev:'resume', wall:+wall.toFixed(2), advanced:+adv.toFixed(2), ratio:+ratio.toFixed(3), verdict:verdict});
    lastHidden=null;
  });
}
'@

$pageA = $shell + @'
<title>A</title>
<h2>A · 우리 오리진 &lt;audio&gt;</h2><p>화면을 한 번 누르세요</p><pre id="out"></pre>
<script>
var KIND='A-same-origin-audio';
'@ + $measure + @'
var a=new Audio('test.opus'); a.loop=true;
document.body.addEventListener('click', function(){
  /* 첫 재생은 **탭이 직접 play() 를 불러야** 한다. 사이에 await 를 끼우면 사용자 활성화가
     소모돼 거절된다. 그래서 여기서 곧바로 부르고 실패는 promise 로 받는다. */
  a.play().then(function(){
    show('재생 시작');
    beginBeat(function(){ return a.currentTime; }, function(o){ o.paused=a.paused; });
  }).catch(function(e){ show('play 실패: '+e); post({ev:'play-fail',err:String(e)}); });
});
</script>
'@

$pageB = $shell + @'
<title>B</title>
<h2>B · 유튜브 1×1 iframe</h2><p>지금 방식 · 화면을 한 번 누르세요</p><pre id="out"></pre>
<div style="width:1px;height:1px;overflow:hidden;position:fixed;left:-9px;top:-9px"><div id="p"></div></div>
<script src="https://www.youtube.com/iframe_api"></script>
<script>
var KIND='B-youtube-iframe';
'@ + $measure + @'
var player=null, ready=false;
function readYt(){ try{ return player.getCurrentTime()||0; }catch(e){ return 0; } }
window.onYouTubeIframeAPIReady=function(){
  /* portal.js 의 createYtPlayer 와 **같은 설정**으로 만든다 — 대조군이 지금 방식이어야 뜻이 있다. */
  player=new YT.Player('p',{ height:'1', width:'1',
    videoId:(new URLSearchParams(location.search)).get('v')||'dQw4w9WgXcQ',
    playerVars:{autoplay:0,controls:0,disablekb:1,playsinline:1,rel:0,cc_load_policy:0,origin:location.origin},
    events:{ onReady:function(){ ready=true; show('플레이어 준비됨'); },
             onError:function(e){ show('유튜브 오류 '+e.data); post({ev:'yt-error',code:e.data}); } }});
};
document.body.addEventListener('click', function(){
  if(!ready){ show('아직 준비 안 됨'); return; }
  player.playVideo(); show('재생 요청');
  beginBeat(readYt, function(o){ try{ o.state=player.getPlayerState(); }catch(e){} });
});
</script>
'@

$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText((Join-Path $Root 'a.html'), $pageA, $utf8)
[IO.File]::WriteAllText((Join-Path $Root 'b.html'), $pageB, $utf8)
Write-Host "[test] 자산 $Root"

# ── 시험 서버 ───────────────────────────────────────────────────────────────
#
# **포트가 이미 잡혀 있으면 여기서 멈춘다.** 안 그러면 우리 서버가 못 붙고 남의 서버가
# 대신 응답하는데, 기기 화면에는 남의 404 만 뜬다. 실제로 그렇게 30분을 날렸다 —
# 앞선 시험에서 남은 파이썬 서버가 같은 포트를 잡고 있었다.
$busy = @(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue)
if ($busy.Count) {
    $who = ($busy | ForEach-Object {
        $p = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
        if ($p) { "$($p.ProcessName)(PID $($p.Id))" } else { "PID $($_.OwningProcess)" }
    }) -join ', '
    throw "포트 $Port 을 이미 $who 이(가) 쓰고 있어요. 끄거나 -Port 로 다른 번호를 주세요."
}

# 파일을 주고, 페이지가 보내는 보고를 한 줄씩 쌓는다. 화면을 눈으로 볼 필요가 없어진다.
$server = Start-Job -ArgumentList $Root, $Port, $LogPath -ScriptBlock {
    param($Root, $Port, $LogPath)
    $listener = [Net.HttpListener]::new()
    $listener.Prefixes.Add("http://localhost:$Port/")
    $listener.Start()
    $mime = @{ '.html' = 'text/html; charset=utf-8'; '.opus' = 'audio/ogg'; '.js' = 'text/javascript' }
    while ($listener.IsListening) {
        $ctx = $listener.GetContext()
        try {
            if ($ctx.Request.HttpMethod -eq 'POST' -and $ctx.Request.Url.AbsolutePath -eq '/report') {
                $body = [IO.StreamReader]::new($ctx.Request.InputStream, [Text.Encoding]::UTF8).ReadToEnd()
                $stamp = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() / 1000.0
                [IO.File]::AppendAllText($LogPath, ('{0:F3} {1}' -f $stamp, $body) + "`n",
                    [Text.UTF8Encoding]::new($false))
                $ctx.Response.StatusCode = 204
            } else {
                $rel = $ctx.Request.Url.AbsolutePath.TrimStart('/')
                if (-not $rel) { $rel = 'a.html' }
                $path = Join-Path $Root $rel
                # 경로를 타고 올라가는 요청은 받지 않는다. 시험용이라도 열어 둘 이유가 없다.
                if ((Test-Path $path) -and -not (Split-Path $rel -Parent)) {
                    $bytes = [IO.File]::ReadAllBytes($path)
                    $ext = [IO.Path]::GetExtension($path)
                    if ($mime.ContainsKey($ext)) { $ctx.Response.ContentType = $mime[$ext] }
                    $ctx.Response.Headers.Add('Cache-Control', 'no-store')
                    $ctx.Response.OutputStream.Write($bytes, 0, $bytes.Length)
                } else { $ctx.Response.StatusCode = 404 }
            }
        } catch { }
        try { $ctx.Response.Close() } catch { }
    }
}
Start-Sleep -Seconds 2

# **떴다고 믿지 말고 물어본다.** `$server.State` 만 보면 아직 'Running' 인 채로 실패가
# 안 드러난다. 우리가 만든 페이지가 그대로 돌아오는지까지 확인해야, 남의 서버가 응답하는
# 경우를 여기서 잡는다.
try {
    $probe = Invoke-WebRequest "http://localhost:$Port/a.html" -UseBasicParsing -TimeoutSec 5
} catch {
    $why = if ($server.State -eq 'Failed') { ($server | Receive-Job 2>&1) -join '; ' } else { $_.Exception.Message }
    throw "시험 서버에 접근이 안 돼요: $why"
}
if ($probe.Content -notmatch 'A-same-origin-audio') {
    throw "포트 $Port 에서 **다른 서버**가 응답하고 있어요. 우리 페이지가 아닙니다."
}
Write-Host '[test] 시험 서버 확인됨'

# 기기의 localhost → 이 PC. **localhost 여야 한다** — `10.0.2.2` 는 보안 컨텍스트가 아니라서
# 서비스워커·PWA 설치가 막히고, 그러면 실제 설치형 앱과 다른 조건을 재게 된다.
Invoke-Adb reverse "tcp:$Port" "tcp:$Port" | Out-Null
Write-Host "[test] adb reverse localhost:$Port"

# ── 한 갈래 돌리기 ──────────────────────────────────────────────────────────
function Get-StartedCount {
    ([regex]::Matches(((Invoke-Adb shell dumpsys audio 2>$null) -join "`n"), 'state:started')).Count
}
function Read-Reports([string]$Kind) {
    if (-not (Test-Path $LogPath)) { return @() }
    Get-Content $LogPath -Encoding UTF8 | ForEach-Object {
        try { $o = ($_ -replace '^[0-9.]+\s+', '') | ConvertFrom-Json } catch { return }
        if ($o.kind -eq $Kind) { $o }
    }
}

function Invoke-Variant([string]$Name, [string]$Page, [string]$Kind) {
    Write-Host ''
    Write-Host "══ $Name ══════════════════════════════"
    # 앞선 갈래가 배경에서 계속 재생되면 다음 측정을 오염시킨다. 크롬을 통째로 껐다 켠다.
    Invoke-Adb shell am force-stop com.android.chrome | Out-Null
    Start-Sleep -Seconds 2
    Invoke-Adb shell am start -a android.intent.action.VIEW -d "http://localhost:$Port/$Page" `
        -n com.android.chrome/com.google.android.apps.chrome.Main | Out-Null
    Start-Sleep -Seconds 8   # B 갈래는 iframe API 를 내려받을 시간이 필요하다

    $w = 540; $h = 1200
    if (((Invoke-Adb shell wm size) -join '') -match '(\d+)x(\d+)') {
        $w = [int]$Matches[1] / 2; $h = [int]$Matches[2] / 2
    }

    # **눌렀다고 바로 포기하지 않는다.** 보고가 파일 전송 뒤에 줄을 서면 몇 초 늦게 온다.
    # 3초만 기다리고 3번 눌러 포기했더니, 실제로는 재생 중인데 '시작 실패' 로 적혔다.
    # 누르는 것과 기다리는 것을 분리한다 — 9초마다 한 번만 다시 누르고, 총 30초까지 본다.
    $started = $false
    foreach ($tick in 1..10) {
        if ($tick -eq 1 -or ($tick % 3) -eq 1) {
            Invoke-Adb shell input tap $w $h | Out-Null
            if ($tick -gt 1) { Write-Host "[test]   아직 조용해요 — 다시 눌러 봅니다" }
        }
        Start-Sleep -Seconds 3
        if (@(Read-Reports $Kind | Where-Object { $_.ev -eq 'start' }).Count) { $started = $true; break }
    }
    if (-not $started) {
        # 첫 실행 안내(로그인·알림 권한)가 덮고 있으면 여기 걸린다. 화면을 남겨 눈으로 보게 한다.
        #
        # `exec-out ... | Set-Content` 를 쓰면 안 된다. PowerShell 5.1 에는 `-AsByteStream` 이
        # 없고, 파이프로 받는 순간 바이트가 텍스트로 해석돼 PNG 가 깨진다. 기기에 찍어서 받아온다.
        $shot = Join-Path $Root "$Name-stuck.png"
        Invoke-Adb shell screencap -p /sdcard/mc-bgtest.png | Out-Null
        Invoke-Adb pull /sdcard/mc-bgtest.png $shot | Out-Null
        Invoke-Adb shell rm -f /sdcard/mc-bgtest.png | Out-Null
        Write-Warning "재생을 시작하지 못했어요. 크롬 첫 실행 화면이 막고 있을 수 있어요: $shot"
        return [pscustomobject]@{ 갈래 = $Name; 판정 = '시작 실패'; 비율 = $null; 플랫폼 = '—' }
    }
    Write-Host '[test]   재생 시작됨'

    $fg = Get-StartedCount
    Invoke-Adb shell input keyevent KEYCODE_HOME | Out-Null
    Write-Host "[test]   홈으로 내보냄 · $Seconds 초 대기"
    $samples = @()
    $slice = [Math]::Max(1, [int]($Seconds / 3))
    foreach ($i in 1..3) { Start-Sleep -Seconds $slice; $samples += (Get-StartedCount) }
    Invoke-Adb shell am start -n com.android.chrome/com.google.android.apps.chrome.Main | Out-Null
    Start-Sleep -Seconds 4

    $resume = @(Read-Reports $Kind | Where-Object { $_.ev -eq 'resume' }) | Select-Object -Last 1
    $verdict = if ($resume) { $resume.verdict } else { '판정 없음' }
    Write-Host "[test]   JS 판정 $verdict · 플랫폼 전경 $fg → 배경 $($samples -join ',')"
    [pscustomobject]@{
        갈래   = $Name
        판정   = $verdict
        비율   = if ($resume) { $resume.ratio } else { $null }
        플랫폼 = "전경 $fg → 배경 $($samples -join ',')"
    }
}

$results = @()
try {
    if ($Only -ne 'B') { $results += Invoke-Variant 'A 우리 오리진 audio' 'a.html' 'A-same-origin-audio' }
    if ($Only -ne 'A') { $results += Invoke-Variant 'B 유튜브 iframe' "b.html?v=$VideoId" 'B-youtube-iframe' }
} finally {
    Invoke-Adb reverse --remove "tcp:$Port" 2>$null | Out-Null
    Stop-Job $server -ErrorAction SilentlyContinue | Out-Null
    Remove-Job $server -Force -ErrorAction SilentlyContinue | Out-Null
    if ($startedEmulator -and -not $KeepEmulator) {
        Write-Host '[test] 에뮬레이터 종료'
        Invoke-Adb emu kill 2>$null | Out-Null
    }
}

Write-Host ''
Write-Host '════════ 결과 ════════'
$results | Format-Table -AutoSize
Write-Host "보고 원본: $LogPath"
Write-Host ''
Write-Host 'PLAYED = 배경에서도 계속 났다 · PAUSED = 멈췄다.'
Write-Host 'A 가 PLAYED 이고 B 가 PAUSED 여야 의미가 있어요. 둘 다 PLAYED 면 에뮬레이터가'
Write-Host '배경 정책을 안 걸고 있는 것이니 결과를 믿지 마세요.'
Write-Host '여기서 PLAYED 라도 실기기 한 대로는 확인해야 해요 — 맨 안드로이드에는 제조사'
Write-Host '배터리 관리가 없습니다. 30분 이상·화면 잠금·절전 모드는 실기기 몫입니다.'
