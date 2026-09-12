# run.ps1 -- Windows launcher for the Prompt Deck pane.
#
# Toggle behavior, scoped to the whole session: find an existing "Prompt Deck" pane and
# focus it, otherwise split the focused pane downward, run the deck binary (absolute
# path, no shell), and label it so the next toggle finds it.
#
# Windows notes (verified against herdr on this machine):
#  - herdr cannot spawn a manifest pane's RELATIVE command on Windows, so we use
#    `pane split` + `pane run <absolute .exe>` + `pane rename` instead of `plugin pane open`.
#  - herdr has no focus-by-id; focusing a specific pane is a `zoom <id> --on`/`--off` cycle.
#  - PowerShell decodes herdr's UTF-8 JSON with the legacy code page unless forced to UTF-8.

$ErrorActionPreference = 'Continue'

$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $Utf8NoBom
$OutputEncoding = $Utf8NoBom

$HerdrBin = if ($env:HERDR_BIN_PATH) { $env:HERDR_BIN_PATH } else { 'herdr' }

function Strip-Verbatim([string]$p) {
    if ($p -and $p.StartsWith('\\?\')) { return $p.Substring(4) }
    return $p
}

$PluginRoot = Strip-Verbatim (Split-Path -Parent $PSScriptRoot)
$DeckBin = Join-Path $PluginRoot 'target\release\prompt-deck.exe'
$DeckTitle = 'Prompt Deck'

function Get-Panes {
    try { return (& $HerdrBin pane list | ConvertFrom-Json).result.panes } catch { return @() }
}

function Find-DeckPane {
    Get-Panes | Where-Object {
        @($_.terminal_title, $_.terminal_title_stripped, $_.title, $_.label) -contains $DeckTitle
    } | Select-Object -First 1
}

function Focus-Pane([string]$paneId) {
    & $HerdrBin pane zoom $paneId --on  *> $null
    & $HerdrBin pane zoom $paneId --off *> $null
}

$existing = Find-DeckPane
if ($existing) {
    Focus-Pane $existing.pane_id
    exit 0
}

$target = Get-Panes | Where-Object { $_.focused } | Select-Object -First 1
if (-not $target) {
    try { $target = (& $HerdrBin pane current | ConvertFrom-Json).result.pane } catch {}
}
if (-not $target) { exit 1 }

$cwd = Strip-Verbatim $target.cwd
# --ratio is the share kept by the pane being split, so the deck gets the remainder.
# 0.9 leaves the agent ~90% and makes the bar as slim as herdr allows.
$splitArgs = @('pane', 'split', '--direction', 'down', '--cwd', $cwd, '--ratio', '0.9', '--focus')
$out = (& $HerdrBin @splitArgs | Out-String)
$newPane = ([regex]'"pane_id":"([^"]+)"').Match($out).Groups[1].Value
if (-not $newPane) { exit 1 }

& $HerdrBin pane rename $newPane $DeckTitle *> $null
& $HerdrBin pane run $newPane "& '$DeckBin' pane --target $($target.pane_id)"
exit 0
