# 로컬 개발 서버 — 실서버를 건드리지 않고 리모컨을 통째로 띄운다.
#
# **왜 있나.** 지금까지는 검증할 때마다 임시 폴더를 만들고 `botsettings.json` 을 손으로 쓰고
# 환경변수를 하나씩 세우고 있었다. 그러다 `MUSICBOT_DEV_SEED` 를 빠뜨려서 **재생 중인 곡이
# 없는 채로** 화면을 보고 "왜 아무것도 안 나오지" 를 반복했다. 그 실수를 없애려고 한 줄로 묶는다.
#
# 디스코드 토큰이 없어도 웹은 완전히 뜬다. 게이트웨이 로그인만 401 로 실패하고
# 리모컨·운영 패널·API 는 다 돌아간다.
#
#   scripts\Dev-Local.ps1              # 띄우기 (씨앗 데이터 포함)
#   scripts\Dev-Local.ps1 -Fresh       # 데이터까지 싹 지우고 새로
#   scripts\Dev-Local.ps1 -Port 8792   # 다른 포트로
#   scripts\Dev-Local.ps1 -NoBuild     # 이미 빌드해 뒀으면 건너뛰기

[CmdletBinding()]
param(
    [int]$Port = 8791,
    # 데이터 폴더를 지우고 시작한다. 마이그레이션이나 첫 실행 경로를 볼 때 쓴다.
    [switch]$Fresh,
    [switch]$NoBuild
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$root = Join-Path $repo '.devrun'
$exe  = Join-Path $repo 'target\release\mc-musicbot.exe'

if ($Fresh -and (Test-Path $root)) {
    Write-Host '[dev] 데이터 폴더를 지웁니다.'
    [IO.Directory]::Delete($root, $true)
}
New-Item -ItemType Directory -Force -Path $root | Out-Null

# 봇 설정. 토큰은 진짜일 필요가 없다 — 게이트웨이만 못 붙고 웹은 다 뜬다.
$settings = Join-Path $root 'botsettings.json'
if (-not (Test-Path $settings)) {
    # **BOM 을 넣으면 안 된다.** `Set-Content -Encoding utf8` 은 BOM 을 붙이고,
    # 그러면 serde 가 파싱에 실패해 토큰이 빈 것으로 읽힌다 (HANDOFF 에 적어 둔 함정).
    $json = @'
{
  "token": "LOCAL_DEV_NO_DISCORD",
  "dataRoot": "data",
  "toolsRoot": "tools"
}
'@
    [IO.File]::WriteAllText($settings, $json, (New-Object Text.UTF8Encoding($false)))
}

# 이미 돌고 있으면 먼저 내린다. 포트가 물려 있으면 새로 뜬 것이 조용히 죽는다.
Get-Process mc-musicbot -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $exe } |
    ForEach-Object { Write-Host "[dev] 돌던 것을 내립니다 (pid $($_.Id))."; Stop-Process $_ -Force }

if (-not $NoBuild) {
    Write-Host '[dev] 빌드 중…'
    Push-Location $repo
    try { cargo build --release | Out-Null } finally { Pop-Location }
}
if (-not (Test-Path $exe)) { throw "빌드 결과가 없어요: $exe" }

$env:MUSICBOT_WEB_URLS   = "http://127.0.0.1:$Port"
$env:MUSICBOT_DEV_LOGIN  = '1'
# **이게 빠지면 화면이 텅 빈다.** 재생 중인 곡·대기열 4곡·투표를 심어 준다(`seed_dev_guild`).
$env:MUSICBOT_DEV_SEED   = '1'
# 호스트 분리는 끈다. 로컬에서는 한 주소로 리모컨과 운영 패널을 다 봐야 한다.
$env:MUSICBOT_ADMIN_HOST  = ''
$env:MUSICBOT_REMOTE_HOST = ''

Write-Host ''
Write-Host "[dev] 리모컨   http://127.0.0.1:$Port/music"
Write-Host "[dev] 운영패널 http://127.0.0.1:$Port/"
Write-Host '[dev] 로그인 화면에서 "로컬 검증 계정으로 입장". 두 사람이 필요하면 "2번 사람으로".'
Write-Host '[dev] 멈추려면 Ctrl+C.'
Write-Host ''

Push-Location $root
try { & $exe } finally { Pop-Location }
