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
    [string]$Remote  = 'bot-host',
    [string]$Root    = '<portable-root>',
    [string]$BuildId = (Get-Date -Format 'yyyyMMdd-HHmm'),
    [string]$TaskName = 'MusicBot Portable'
)

$ErrorActionPreference = 'Stop'

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

if ($swap -notmatch 'HEALTHY=True') { throw '새 프로세스가 응답하지 않아요. 로그를 확인하세요.' }
if ($swap -notmatch [Regex]::Escape("SHA=$localHash")) { throw '교체 뒤 해시가 달라요.' }

Write-Host "[deploy] 완료 — 빌드 $BuildId"
