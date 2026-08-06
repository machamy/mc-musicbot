# 이 PC의 현재 LAN 주소를 NAS host-registrar에 등록한다.
#
# 관리자(musicbot.example.com)와 리모컨(music.example.com)이 서로 다른 호스트명을 쓰지만
# 같은 프로세스의 같은 포트(8693)를 가리키므로, 두 호스트명을 같은 대상으로 등록한다.
#
# 설정 파일(data\registrar.json)은 호스트 로컬 파일이다.
# 업데이트가 보존하며 포터블 매니페스트와 Git에 절대 넣지 않는다.
#
#   .\Register-RemoteHosts.ps1 -ConfigPath data\registrar.json

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ConfigPath,

    # 등록에 실패해도 봇 기동은 계속되어야 하므로 기본은 경고만 남긴다.
    [switch]$FailOnError
)

$ErrorActionPreference = 'Stop'

function Write-Step($message) { Write-Host "[registrar] $message" }
function Write-Warn($message) { Write-Host "[registrar] $message" -ForegroundColor Yellow }

function Get-LanAddress {
    # 등록기는 RFC1918 주소만 받는다. 링크로컬/APIPA/가상 어댑터를 걸러낸다.
    $candidates = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        Where-Object {
            $_.IPAddress -notlike '127.*' -and
            $_.IPAddress -notlike '169.254.*' -and
            $_.PrefixOrigin -ne 'WellKnown' -and
            (
                $_.IPAddress -like '192.168.*' -or
                $_.IPAddress -like '10.*' -or
                $_.IPAddress -match '^172\.(1[6-9]|2[0-9]|3[01])\.'
            )
        }

    if (-not $candidates) { return $null }

    # 기본 경로로 나가는 인터페이스를 우선한다. 없으면 메트릭이 가장 낮은 것.
    $defaultIf = (Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue |
        Sort-Object RouteMetric, InterfaceMetric |
        Select-Object -First 1).InterfaceIndex

    if ($defaultIf) {
        $onDefault = $candidates | Where-Object { $_.InterfaceIndex -eq $defaultIf } | Select-Object -First 1
        if ($onDefault) { return $onDefault.IPAddress }
    }

    return ($candidates | Sort-Object InterfaceMetric | Select-Object -First 1).IPAddress
}

if (-not (Test-Path -LiteralPath $ConfigPath)) {
    Write-Warn "설정 파일이 없습니다: $ConfigPath  (scripts\registrar.sample.json 을 복사해서 만드세요)"
    if ($FailOnError) { exit 1 }
    exit 0
}

try {
    $config = Get-Content -LiteralPath $ConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
}
catch {
    Write-Warn "설정 파일을 읽을 수 없습니다: $($_.Exception.Message)"
    if ($FailOnError) { exit 1 }
    exit 0
}

$registrarUrl = $config.registrarUrl
$secret = $config.secret
$port = if ($config.port) { [int]$config.port } else { 8693 }
$hosts = @($config.hosts)

if (-not $registrarUrl -or -not $secret -or $hosts.Count -eq 0) {
    Write-Warn 'registrarUrl / secret / hosts 중 비어 있는 항목이 있어 등록을 건너뜁니다.'
    if ($FailOnError) { exit 1 }
    exit 0
}

$ip = if ($config.ip) { $config.ip } else { Get-LanAddress }
if (-not $ip) {
    Write-Warn 'LAN IPv4 주소를 찾지 못해 등록을 건너뜁니다.'
    if ($FailOnError) { exit 1 }
    exit 0
}

Write-Step "대상 $ip`:$port"

$failed = 0
foreach ($hostname in $hosts) {
    $uri = "$($registrarUrl.TrimEnd('/'))/register/$hostname"
    $body = @{ ip = $ip; port = $port; secret = $secret } | ConvertTo-Json -Compress
    try {
        Invoke-RestMethod -Method Post -Uri $uri -ContentType 'application/json' `
            -Body $body -TimeoutSec 10 | Out-Null
        Write-Step "$hostname  -> OK"
    }
    catch {
        $failed++
        # 비밀값이 섞이지 않도록 예외 메시지만 남긴다.
        Write-Warn "$hostname  -> 실패: $($_.Exception.Message)"
    }
}

if ($failed -gt 0 -and $FailOnError) { exit 1 }
exit 0
