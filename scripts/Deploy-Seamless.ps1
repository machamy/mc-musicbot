# 끊김을 최소로 줄이는 배포 (V3 §24).
#
# 예전 방식은 이랬다.
#     Stop-Process -Force  →  scp exe  →  Start-ScheduledTask
# 이러면 **복사하는 내내 서버가 죽어 있다.** 실측으로 cloudflared 가
# `dial tcp <bot-host-ip>:8693: i/o timeout` 을 2분 넘게 뱉었다.
#
# 여기서는 순서를 바꾼다.
#     1. 새 exe 를 옆자리(.next)에 먼저 복사한다 — 봇은 계속 돌고 있다.
#     2. 해시를 대조한다. 틀리면 아무것도 건드리지 않고 멈춘다.
#     3. 종료 신호를 보낸다. 봇이 재생 위치를 저장하고 음성에서 깨끗이 빠진다.
#     4. 파일을 바꿔치고 곧바로 띄운다.
# 멈춘 시간이 "복사 + 기동" 에서 "기동" 만 남는다.
#
# 봇은 다음 기동에서 저장한 지점부터 이어서 튼다. 접속 중인 브라우저에는
# 미리 `server.restarting` 이 나가서 오류 화면 대신 안내가 뜬다.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$LocalExe,
    # **버전은 반드시 준다.** 규칙이다 — 모든 빌드·배포에는 버전 번호와 패치노트가 있어야 한다.
    # 날짜 자동값을 쓰면 "무엇이 바뀐 배포인지"가 아무 데도 안 남는다.
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Remote  = 'bot-host',
    [string]$Root    = '<portable-root>',
    [string]$TaskName = 'MusicBot Portable',
    [string]$ChangelogPath = "$PSScriptRoot\..\docs\CHANGELOG.md"
)

$ErrorActionPreference = 'Stop'

# ── 규칙 검사: 이 버전의 패치노트가 있어야 배포한다 ────────────────────────
# 패치노트 없이 나간 빌드는 몇 주 뒤에 "이게 언제 뭐가 바뀐 거지"가 되고,
# 인앱 패치노트(§30)도 이 파일을 그대로 읽으므로 화면에서도 빈칸이 된다.
if (-not (Test-Path $ChangelogPath)) { throw "패치노트 파일이 없어요: $ChangelogPath" }
$changelog = [IO.File]::ReadAllText((Resolve-Path $ChangelogPath), [Text.Encoding]::UTF8)
if ($changelog -notmatch [Regex]::Escape("## $Version")) {
    throw @"
패치노트에 '## $Version' 항목이 없어요.
docs/CHANGELOG.md 맨 위에 이 버전 항목을 먼저 쓰고 다시 실행하세요.
(모든 빌드·배포에는 버전 번호와 패치노트가 있어야 한다 — 프로젝트 규칙)
"@
}
$BuildId = $Version
Write-Host "[deploy] 버전 $Version · 패치노트 확인됨"

function Invoke-Remote([string]$Script) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($Script))
    ssh -o BatchMode=yes $Remote "powershell -NoProfile -EncodedCommand $encoded"
}

if (-not (Test-Path $LocalExe)) { throw "빌드 결과가 없어요: $LocalExe" }
$localHash = (Get-FileHash $LocalExe -Algorithm SHA256).Hash
Write-Host "[deploy] 로컬 SHA256 $localHash"

# 1. 봇이 도는 채로 옆자리에 복사한다.
$rootFwd = $Root.Replace('\', '/')
Write-Host '[deploy] 새 exe 를 .next 로 복사하는 중 (서비스는 계속 돌아요)'
scp -o BatchMode=yes $LocalExe "${Remote}:$rootFwd/bot-mk2/mc-musicbot.exe.next"
if ($LASTEXITCODE -ne 0) { throw '복사 실패 — 아무것도 바꾸지 않았어요.' }

# 2. 해시 대조. 여기서 틀리면 서비스를 건드리기 전에 멈춘다.
$verify = Invoke-Remote @"
`$next = Join-Path '$Root' 'bot-mk2\mc-musicbot.exe.next'
if (-not (Test-Path `$next)) { 'MISSING'; exit }
(Get-FileHash `$next -Algorithm SHA256).Hash
"@
$remoteHash = ($verify | Select-Object -Last 1).Trim()
if ($remoteHash -ne $localHash) {
    throw "복사본 해시가 달라요. 배포를 중단했어요. (원격 $remoteHash)"
}
Write-Host '[deploy] 해시 일치 — 여기서부터가 멈춤 구간이에요'

# 3~4. 신호 → 교체 → 기동. 한 번의 원격 왕복으로 끝낸다.
$swap = Invoke-Remote @"
`$root = '$Root'
`$exe  = Join-Path `$root 'bot-mk2\mc-musicbot.exe'
`$next = "`$exe.next"
`$sw = [Diagnostics.Stopwatch]::StartNew()

# 종료 신호. CloseMainWindow 가 콘솔 앱에는 안 먹을 수 있어서, 안 죽으면 강제로 간다.
# **강제로 가더라도 앞서 저장한 재생 위치는 남아 있다** — 그게 이 설계의 요점이다.
`$procs = Get-Process mc-musicbot -ErrorAction SilentlyContinue
foreach (`$p in `$procs) { try { `$null = `$p.CloseMainWindow() } catch {} }
`$deadline = (Get-Date).AddSeconds(8)
while ((Get-Date) -lt `$deadline -and (Get-Process mc-musicbot -ErrorAction SilentlyContinue)) {
    Start-Sleep -Milliseconds 200
}
`$left = Get-Process mc-musicbot -ErrorAction SilentlyContinue
if (`$left) { `$left | Stop-Process -Force; Start-Sleep -Milliseconds 400 }

Move-Item -Path `$next -Destination `$exe -Force
[IO.File]::WriteAllText((Join-Path `$root 'BUILD_ID.txt'), '$BuildId', (New-Object Text.UTF8Encoding(`$false)))
Start-ScheduledTask -TaskName '$TaskName'

# 웹이 응답할 때까지가 실제 멈춤 시간이다.
`$ok = `$false
`$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt `$deadline) {
    try {
        if ((Invoke-WebRequest -UseBasicParsing http://localhost:8693/healthz -TimeoutSec 3).StatusCode -eq 200) {
            `$ok = `$true; break
        }
    } catch { Start-Sleep -Milliseconds 400 }
}
`$sw.Stop()
"SHA=" + (Get-FileHash `$exe -Algorithm SHA256).Hash
"DOWN_MS=" + [int]`$sw.Elapsed.TotalMilliseconds
"HEALTHY=" + `$ok
"@

$swap | ForEach-Object { Write-Host "[deploy] $_" }

# **배열에 -notmatch 를 쓰면 안 된다.** 배열에서는 필터로 동작해서 "안 맞는 줄들"을
# 돌려주는데, 그게 비어 있지 않으면 참이 된다. 그래서 배포가 멀쩡히 끝났는데도
# 실패로 죽었다. 한 문자열로 합쳐서 본다.
$report = ($swap | Out-String)
if ($report -notmatch 'HEALTHY=True') { throw '새 프로세스가 응답하지 않아요. 로그를 확인하세요.' }
if ($report -notmatch [Regex]::Escape("SHA=$localHash")) { throw '교체 뒤 해시가 달라요.' }

Write-Host "[deploy] 완료 — 빌드 $BuildId"
