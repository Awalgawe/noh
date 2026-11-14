param(
    [switch]$Gui,
    [switch]$Offline,
    [switch]$NoBundle,
    [string]$FfmpegPath = $env:NOH_FFMPEG,
    # A GUI bundle needs the language packs built by tools/build_syllable_dicts.py
    # (Task 44); defaults to NOH_LANGUAGES.
    [string]$Languages = $env:NOH_LANGUAGES
)
$ErrorActionPreference = 'Stop'
$taskCargo = (Get-Command cargo -ErrorAction Stop).Source
if ($FfmpegPath -and -not $NoBundle) {
    $FfmpegPath = (Resolve-Path -LiteralPath $FfmpegPath -ErrorAction Stop).Path
}
Push-Location -LiteralPath (Join-Path $PSScriptRoot '..')
try {
    # Compatibility entry point: all build and publication logic lives in Rust.
    $taskArgs = @('run', '--locked')
    if ($Offline) { $taskArgs += '--offline' }
    $taskArgs += @('--example', 'dev', '--', 'build')
    if ($Gui) { $taskArgs += '--gui' }
    if ($Offline) { $taskArgs += '--offline' }
    if ($NoBundle) { $taskArgs += '--no-bundle' }
    if ($FfmpegPath) { $taskArgs += @('--ffmpeg', $FfmpegPath) }
    if ($Gui -and -not $NoBundle) {
        if (-not $Languages) { throw 'A GUI bundle needs -Languages (or NOH_LANGUAGES): the folder written by py -I tools/build_syllable_dicts.py --data <D:\data> --out <folder>.' }
        $env:NOH_LANGUAGES = (Resolve-Path -LiteralPath $Languages -ErrorAction Stop).Path
    }
    & $taskCargo @taskArgs
    if ($LASTEXITCODE -ne 0) { throw "Rust build task failed ($LASTEXITCODE)." }
} finally { Pop-Location }
