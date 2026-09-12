# fetch-or-build.ps1 -- herdr [[build]] step for prompt-deck (Windows).
#
# Fast path: download the prebuilt binary that matches THIS source's declared version
# and platform from the GitHub release, verify its SHA-256, and install it at
# target\release\prompt-deck.exe. The match is by VERSION, not commit.
# Fallback: on ANY miss (no asset, network error, checksum mismatch, unmapped
# platform, no cargo) print a clear notice and build from source with cargo.
#
# Windows PowerShell 5.1 compatible -- the in-box shell, so installing needs no
# extra tooling. Overridable via env (PD_REPO_ROOT / PD_CARGO_TOML / PD_OUT /
# PD_BASE_URL) so the logic can be exercised against a mock.

$ErrorActionPreference = 'Stop'

$Repo = 'matdac12/herdr-prompt-deck'

$RepoRoot = if ($env:PD_REPO_ROOT) { $env:PD_REPO_ROOT } else { Join-Path $PSScriptRoot '..' }
$CargoToml = if ($env:PD_CARGO_TOML) { $env:PD_CARGO_TOML } else { Join-Path $RepoRoot 'Cargo.toml' }
$Out = if ($env:PD_OUT) { $env:PD_OUT } else { Join-Path $RepoRoot 'target\release\prompt-deck.exe' }
$BaseUrl = if ($env:PD_BASE_URL) { $env:PD_BASE_URL } else { "https://github.com/$Repo/releases/download" }

function Build-FromSource {
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) {
        [Console]::Error.WriteLine("prompt-deck needs Rust 1.85+ to build, but cargo was not found. Install Rust from https://rustup.rs then re-run: herdr plugin install $Repo")
        exit 1
    }
    & cargo build --release
    exit $LASTEXITCODE
}

function Invoke-Fallback {
    param([string]$Reason)
    [Console]::Error.WriteLine("prompt-deck: $Reason - building from source instead.")
    if ($script:TmpDir -and (Test-Path $script:TmpDir)) {
        Remove-Item -Recurse -Force $script:TmpDir -ErrorAction SilentlyContinue
    }
    Build-FromSource
}

function Get-Sha256Hex {
    param([string]$Path)
    try {
        return (Get-FileHash -Algorithm SHA256 -Path $Path -ErrorAction Stop).Hash.ToLowerInvariant()
    } catch {
        try {
            $out = & certutil -hashfile $Path SHA256 2>$null
        } catch {
            return $null
        }
        if ($LASTEXITCODE -ne 0 -or -not $out) { return $null }
        $hashLine = $out | Where-Object { $_ -match '^[0-9a-fA-F]{64}$' } | Select-Object -First 1
        if ($hashLine) { return $hashLine.Trim().ToLowerInvariant() }
        return $null
    }
}

function Invoke-Download {
    param([string]$Url, [string]$Dest)
    try {
        # Windows PowerShell 5.1 may negotiate TLS 1.0 by default; force 1.2 so GitHub's
        # release CDN accepts the request instead of needlessly falling back to a build.
        try {
            [Net.ServicePointManager]::SecurityProtocol =
                [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        } catch {}
        Invoke-WebRequest -Uri $Url -OutFile $Dest -UseBasicParsing -ErrorAction Stop
        return $true
    } catch {
        return $false
    }
}

# --- resolve the target triple ----------------------------------------------------------------
$Arch = $env:PROCESSOR_ARCHITECTURE
$Triple = $null
if ($Arch -eq 'AMD64') { $Triple = 'x86_64-pc-windows-msvc' }
if (-not $Triple) { Invoke-Fallback "no prebuilt binary for Windows/$Arch" }

# --- read the version this source declares ----------------------------------------------------
$Version = $null
if (Test-Path $CargoToml) {
    $match = Select-String -Path $CargoToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($match) { $Version = $match.Matches[0].Groups[1].Value }
}
if (-not $Version) { Invoke-Fallback "could not read version from $CargoToml" }

$Asset = "prompt-deck-$Triple.exe"

$script:TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("pd-fob-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $script:TmpDir -Force | Out-Null

# Transparency only: note when this checkout is ahead of the release commit.
$AheadNote = ''
try {
    $git = Get-Command git -ErrorAction SilentlyContinue
    if ($git) {
        & git -C $RepoRoot rev-parse --is-inside-work-tree *> $null
        if ($LASTEXITCODE -eq 0) {
            $headRev = (& git -C $RepoRoot rev-parse HEAD 2>$null)
            if (-not $headRev) { $headRev = 'nohead' }
            $commitFile = Join-Path $script:TmpDir 'COMMIT'
            if (Invoke-Download "$BaseUrl/v$Version/COMMIT" $commitFile) {
                $releaseCommit = (Get-Content $commitFile -Raw -ErrorAction SilentlyContinue)
                if ($releaseCommit) { $releaseCommit = $releaseCommit.Trim() }
                if ($releaseCommit -and ($headRev -ne $releaseCommit)) {
                    $AheadNote = " Note: this checkout ($headRev) is ahead of the v$Version release commit ($releaseCommit)."
                }
            }
        }
    }
} catch {}

$BinUrl = "$BaseUrl/v$Version/$Asset"
$SumsUrl = "$BaseUrl/v$Version/SHA256SUMS"
$TmpBin = Join-Path $script:TmpDir $Asset
$TmpSums = Join-Path $script:TmpDir 'SHA256SUMS'

if (-not (Invoke-Download $BinUrl $TmpBin)) { Invoke-Fallback "prebuilt binary not available for v$Version ($Asset)" }
if (-not (Invoke-Download $SumsUrl $TmpSums)) { Invoke-Fallback "checksums not available for v$Version" }

# Expected hash = the SHA256SUMS line for our asset (accept text or binary marker).
$Expected = $null
$assetPattern = [regex]::Escape($Asset)
Get-Content $TmpSums | ForEach-Object {
    if (-not $Expected -and $_ -match "^([0-9a-fA-F]{64}) [ *]${assetPattern}`$") {
        $Expected = $Matches[1].ToLowerInvariant()
    }
}
if (-not $Expected) { Invoke-Fallback "no checksum listed for $Asset" }

$Actual = Get-Sha256Hex $TmpBin
if (-not $Actual) { Invoke-Fallback 'no SHA-256 tool (Get-FileHash/certutil) available' }
if ($Actual -ne $Expected) { Invoke-Fallback "checksum mismatch for $Asset (expected $Expected, got $Actual)" }

$OutDir = Split-Path -Parent $Out
if ($OutDir -and -not (Test-Path $OutDir)) {
    New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
}
Move-Item -Force $TmpBin $Out
[Console]::Out.WriteLine("prompt-deck: installed prebuilt v$Version ($Triple), verified SHA-256.$AheadNote")
Remove-Item -Recurse -Force $script:TmpDir -ErrorAction SilentlyContinue
exit 0
