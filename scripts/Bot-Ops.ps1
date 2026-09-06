# 봇 호스트를 들여다보는 운영 도구.
#
# 왜 있나 — 사고가 났을 때 매번 이런 걸 손으로 쳤다.
#
#     $enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($cmd))
#     ssh aux-server "powershell -NoProfile -EncodedCommand $enc"
#
# 그리고 돌아온 한글이 전부 깨졌다(`ó�� ����ϱ�`). 원격 PowerShell 이 CLIXML 로 감싸
# 보내면서 콘솔 코드페이지를 타기 때문이다. 그래서 **결과를 UTF-8 base64 로 감싸
# 돌려받는다** — 중간에 어떤 코드페이지를 거치든 글자가 안 깨진다.
#
# 쓰는 법
#     .\scripts\Bot-Ops.ps1 status              지금 상태 한눈에
#     .\scripts\Bot-Ops.ps1 logs -Lines 80      최근 로그
#     .\scripts\Bot-Ops.ps1 logs -Level Error -Category Playback
#     .\scripts\Bot-Ops.ps1 logs -Date 20260904 -Match '403'
#     .\scripts\Bot-Ops.ps1 failures -Days 14   재생 실패를 사유별로 집계
#     .\scripts\Bot-Ops.ps1 tools               yt-dlp / deno / ffmpeg 상태
#
# 읽기만 한다. 봇을 멈추거나 파일을 바꾸지 않는다 — 배포는 Deploy-Seamless.ps1 이 한다.

[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('status', 'logs', 'failures', 'tools')]
    [string]$Command = 'status',

    # 배포와 같은 환경변수를 쓴다. 호스트마다 다른 값을 저장소에 박지 않는다.
    [string]$Remote = $(if ($env:MUSICBOT_DEPLOY_REMOTE) { $env:MUSICBOT_DEPLOY_REMOTE } else { 'bot-host' }),
    [string]$Root   = $(if ($env:MUSICBOT_DEPLOY_ROOT) { $env:MUSICBOT_DEPLOY_ROOT } else { 'C:\musicbot-portable' }),

    [int]$Lines = 40,
    [int]$Days = 7,
    [string]$Level = '',
    [string]$Category = '',
    [string]$Date = '',
    [string]$Match = ''
)

$ErrorActionPreference = 'Stop'

# ── 원격 실행 ────────────────────────────────────────────────────────────────
#
# 결과를 UTF-8 base64 로 감싸 돌려받는다. 이걸 안 하면 한글이 깨진다(위 주석 참고).
# 원격 쪽 오류도 문자열로 받아 와야 여기서 보여 줄 수 있으므로 try/catch 로 감싼다.
function Invoke-Remote([string]$Script) {
    $wrapped = @"
`$ErrorActionPreference = 'Stop'
# 진행률 표시를 끈다. 안 끄면 원격이 CLIXML 로 감싼 진행 레코드를 stderr 로 쏟아
# 우리 화면을 덮는다 — 결과와 섞여 읽을 수 없게 된다.
`$ProgressPreference = 'SilentlyContinue'
`$out = try { & { $Script } 2>&1 | Out-String } catch { "원격 오류: " + `$_.Exception.Message }
[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(`$out))
"@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($wrapped))
    $reply = ssh -o BatchMode=yes $Remote "powershell -NoProfile -EncodedCommand $encoded"
    if ($LASTEXITCODE -ne 0) { throw "ssh 실패 ($Remote). PowerShell 에서 돌리고 있나요? Git Bash 의 ssh 는 키를 못 찾습니다 (HANDOFF.md §배포)." }
    # CLIXML 진행률 잡음이 섞여 오므로 base64 로 보이는 마지막 줄만 고른다.
    $payload = $reply | Where-Object { $_ -match '^[A-Za-z0-9+/=]+$' -and $_.Length -gt 16 } | Select-Object -Last 1
    if (-not $payload) { throw "원격에서 읽을 수 있는 응답이 없어요." }
    [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($payload))
}

# 원격 스크립트 안에서 쓸 공통 머리말. 경로를 한 곳에서만 만든다.
$prelude = @"
`$root = '$Root'
`$logs = Join-Path `$root 'data\logs'
"@

switch ($Command) {

    'status' {
        Write-Host "[$Remote] 봇 현황" -ForegroundColor Cyan
        Invoke-Remote @"
$prelude
`$p = Get-Process -Name mc-musicbot -ErrorAction SilentlyContinue
if (`$p) {
    `$up = (Get-Date) - `$p.StartTime
    "프로세스   : PID `$(`$p.Id) · 기동 `$(`$p.StartTime.ToString('MM-dd HH:mm')) · `$([int]`$up.TotalHours)시간 `$(`$up.Minutes)분째"
} else {
    "프로세스   : **안 돌고 있어요**"
}
"빌드       : " + (Get-Content (Join-Path `$root 'BUILD_ID.txt') -ErrorAction SilentlyContinue)
`$task = (Get-ScheduledTask -TaskName 'MusicBot Portable' -ErrorAction SilentlyContinue).State
"예약작업   : `$task"

# 안에서 되는지 / 밖에서 되는지를 갈라 본다. 이게 갈리면 방화벽·네트워크 프로필 문제다.
try { `$c = (Invoke-WebRequest 'http://localhost:8693/' -UseBasicParsing -TimeoutSec 8).StatusCode; "웹(안쪽)   : HTTP `$c" }
catch { "웹(안쪽)   : 응답 없음 — 봇이 떠 있어도 웹이 죽었을 수 있어요" }
`$listen = Get-NetTCPConnection -State Listen -LocalPort 8693 -ErrorAction SilentlyContinue
"리슨       : " + `$(if (`$listen) { (`$listen | Select-Object -First 1).LocalAddress + ':8693' } else { '없음' })
`$prof = Get-NetConnectionProfile | Select-Object -First 1
"네트워크   : `$(`$prof.InterfaceAlias) = `$(`$prof.NetworkCategory)" + `$(if (`$prof.NetworkCategory -eq 'Public') { '  ← Public 이면 방화벽이 밖에서 오는 연결을 막아요' } else { '' })

# 스테이징이 남아 있으면 다음 재시작에 갈아 끼워진다. 6일간 방치된 적이 있다.
`$next = Join-Path `$root 'bot-mk2\mc-musicbot.exe.next'
if (Test-Path `$next) { "대기중 배포 : " + (Get-Item `$next).LastWriteTime + " (다음 재시작에 적용돼요)" }

`$today = Join-Path `$logs ('mc-musicbot-' + (Get-Date -Format 'yyyyMMdd') + '.jsonl')
if (Test-Path `$today) {
    `$n = 0; `$err = 0
    foreach (`$l in [IO.File]::ReadLines(`$today)) { `$n++; if (`$l -match '"level":"Error"') { `$err++ } }
    "오늘 로그   : `$n 줄 (오류 `$err) · 마지막 기록 " + (Get-Item `$today).LastWriteTime.ToString('HH:mm:ss')
} else { "오늘 로그   : 없음" }
"@
    }

    'logs' {
        $day = if ($Date) { $Date } else { (Get-Date -Format 'yyyyMMdd') }
        Write-Host "[$Remote] 로그 $day (최근 $Lines 줄)" -ForegroundColor Cyan
        Invoke-Remote @"
$prelude
`$f = Join-Path `$logs 'mc-musicbot-$day.jsonl'
if (-not (Test-Path `$f)) { "그 날짜 로그가 없어요: $day"; return }
`$hits = New-Object Collections.Generic.List[string]
foreach (`$l in [IO.File]::ReadLines(`$f)) {
    if ('$Level'    -and `$l -notmatch ('"level":"' + '$Level' + '"')) { continue }
    if ('$Category' -and `$l -notmatch ('"category":"' + '$Category' + '"')) { continue }
    if ('$Match'    -and `$l -notmatch [regex]::Escape('$Match')) { continue }
    try { `$o = `$l | ConvertFrom-Json } catch { continue }
    `$hits.Add(('{0} {1,-5} {2,-10} {3}' -f `$o.timestamp.Substring(11,8), `$o.level, `$o.category, `$o.message))
}
"걸린 줄 `$(`$hits.Count) 개 중 마지막 $Lines 개"
if (`$hits.Count -gt $Lines) { `$hits[(`$hits.Count - $Lines)..(`$hits.Count - 1)] } else { `$hits }
"@
    }

    'failures' {
        Write-Host "[$Remote] 재생 실패 집계 (최근 $Days 일)" -ForegroundColor Cyan
        Invoke-Remote @"
$prelude
`$since = (Get-Date).AddDays(-$Days)
`$kinds = @{}; `$ids = @{}; `$byDay = @{}
foreach (`$f in Get-ChildItem `$logs -Filter 'mc-musicbot-*.jsonl') {
    `$stamp = `$f.BaseName.Replace('mc-musicbot-','')
    `$d = [datetime]::ParseExact(`$stamp,'yyyyMMdd',`$null)
    if (`$d -lt `$since.Date) { continue }
    foreach (`$l in [IO.File]::ReadLines(`$f.FullName)) {
        if (`$l -notmatch 'Playback failed') { continue }
        `$byDay[`$stamp] = 1 + `$byDay[`$stamp]
        if (`$l -match '\[youtube\] ([A-Za-z0-9_\-]{6,15}):') { `$ids[`$Matches[1]] = 1 + `$ids[`$Matches[1]] }
        # 사유는 코드의 fail_code 표와 같은 순서로 본다 (media/ytdlp.rs).
        `$k = switch -Regex (`$l) {
            'live stream recording is not available' { '다시보기 없음'; break }
            'processing this video'                  { '처리 중'; break }
            'Video unavailable'                      { '영상 없음'; break }
            'Private video'                          { '비공개'; break }
            'Music Premium'                          { '프리미엄 전용'; break }
            'confirm your age'                       { '연령 확인'; break }
            'removed for violating'                  { '약관 위반 삭제'; break }
            'Requested format'                       { '형식 없음'; break }
            '429|Too Many Requests'                  { '요청 과다'; break }
            '403|Forbidden'                          { '403'; break }
            '초과해 중단'                             { '시간 초과'; break }
            default                                  { '기타' }
        }
        `$kinds[`$k] = 1 + `$kinds[`$k]
    }
}
`$total = (`$kinds.Values | Measure-Object -Sum).Sum
if (-not `$total) { "재생 실패가 한 건도 없어요."; return }
"총 `$total 건 · 고유 영상 `$(`$ids.Count) 개"
""
"사유별"
`$kinds.GetEnumerator() | Sort-Object Value -Descending | ForEach-Object { '  {0,-14} {1}' -f `$_.Key, `$_.Value }
""
"날짜별"
`$byDay.GetEnumerator() | Sort-Object Name | ForEach-Object { '  {0}  {1}' -f `$_.Key, `$_.Value }
`$repeat = `$ids.GetEnumerator() | Where-Object { `$_.Value -gt 1 } | Sort-Object Value -Descending
if (`$repeat) {
    ""
    "같은 영상이 되풀이 실패 (많으면 자동재생이 죽은 영상을 계속 집는다는 뜻)"
    `$repeat | Select-Object -First 10 | ForEach-Object { '  {0}  {1}회' -f `$_.Key, `$_.Value }
}
"@
    }

    'tools' {
        Write-Host "[$Remote] 외부 도구" -ForegroundColor Cyan
        Invoke-Remote @"
$prelude
`$tools = Join-Path `$root 'tools'
foreach (`$n in 'yt-dlp.exe','deno.exe','ffmpeg.exe') {
    `$p = Join-Path `$tools `$n
    if (Test-Path `$p) {
        `$v = switch (`$n) {
            'yt-dlp.exe' { (& `$p --version 2>`$null | Select-Object -First 1) }
            'deno.exe'   { (& `$p --version 2>`$null | Select-Object -First 1) }
            default      { '' }
        }
        '{0,-12} 있음  {1}' -f `$n, `$v
    } else { '{0,-12} **없음**' -f `$n }
}
# yt-dlp 가 얼마나 묵었는지. 45일이 넘으면 봇도 로그로 경고한다 (media/tools.rs).
`$ver = & (Join-Path `$tools 'yt-dlp.exe') --version 2>`$null | Select-Object -First 1
if (`$ver -match '^(\d{4})\.(\d{1,2})\.(\d{1,2})') {
    `$age = ((Get-Date) - (Get-Date -Year `$Matches[1] -Month `$Matches[2] -Day `$Matches[3])).Days
    "yt-dlp 나이 : `$age 일" + `$(if (`$age -ge 45) { '  ← 오래됐어요. 유튜브가 바뀌면 곡을 못 받아요' } else { '' })
}
`$conf = Join-Path `$tools 'yt-dlp.conf'
if (Test-Path `$conf) { "yt-dlp.conf : " + ((Get-Content `$conf -Raw).Trim()) }
"@
    }
}
