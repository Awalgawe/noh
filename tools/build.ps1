param(
    [switch]$Gui,
    [switch]$Offline,
    [switch]$NoBundle,
    [string]$FfmpegPath = $env:NOH_FFMPEG
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
    & $taskCargo @taskArgs
    if ($LASTEXITCODE -ne 0) { throw "Rust build task failed ($LASTEXITCODE)." }
} finally { Pop-Location }
