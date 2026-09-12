# run.ps1 -- Windows entrypoint for the Prompt Deck toggle action.
#
# All the logic lives in the binary's `launch` mode (cross-platform, testable); this
# script only locates the binary. herdr cannot reliably run a manifest action's
# relative program on Windows, so the action's command locates this script through
# herdr's own plugin root and runs it by absolute path (see herdr-plugin.toml).

$ErrorActionPreference = 'Continue'

$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $Utf8NoBom
$OutputEncoding = $Utf8NoBom

function Strip-Verbatim([string]$p) {
    if ($p -and $p.StartsWith('\\?\')) { return $p.Substring(4) }
    return $p
}

$PluginRoot = Strip-Verbatim (Split-Path -Parent $PSScriptRoot)
$Bin = Join-Path $PluginRoot 'target\release\prompt-deck.exe'

if (-not (Test-Path $Bin)) {
    [Console]::Error.WriteLine("prompt-deck: $Bin not found. Run: cargo build --release")
    exit 1
}

& $Bin launch
exit $LASTEXITCODE
