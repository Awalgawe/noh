param(
    [Parameter(Mandatory=$true)][string]$Video,
    [Parameter(Mandatory=$true)][string]$Wav,
    [string]$OutputPath,
    [double]$FadeIn = 0,
    [double]$FadeOut = 0,
    [switch]$PartialFades
)
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$taskExe = Join-Path $taskRoot 'dist\noh\bin\noh-cli.exe'
if (-not (Test-Path -LiteralPath $taskExe -PathType Leaf)) { $taskExe = Join-Path $taskRoot 'target\release\noh.exe' }
if (-not (Test-Path -LiteralPath $taskExe -PathType Leaf)) { throw "Executable not found: $taskExe" }
$taskLogDir = Join-Path $taskRoot 'logs'
[void](New-Item -ItemType Directory -Path $taskLogDir -Force)
$taskLog = Join-Path $taskLogDir ('export-' + [guid]::NewGuid().ToString('N') + '.log')
$taskEvents = $taskLog + '.events.jsonl'
$taskCancel = $taskLog + '.cancel'
Set-Content -LiteralPath $taskLog -Value 'Starting export...' -Encoding UTF8
$taskUi = Join-Path $PSScriptRoot 'watch-export.ps1'
$taskUiArgs = @('-NoProfile', '-STA', '-WindowStyle', 'Hidden', '-ExecutionPolicy', 'Bypass', '-File', ('"' + $taskUi + '"'), '-LogPath', ('"' + $taskLog + '"'), '-ExportPid', $PID, '-EventsPath', ('"' + $taskEvents + '"'), '-CancelPath', ('"' + $taskCancel + '"'))
[void](Start-Process -FilePath 'powershell.exe' -ArgumentList $taskUiArgs -WindowStyle Hidden)
$taskArgs = @($Video, $Wav, '--fade-in', $FadeIn.ToString([cultureinfo]::InvariantCulture), '--fade-out', $FadeOut.ToString([cultureinfo]::InvariantCulture), '--events', $taskEvents, '--cancel-file', $taskCancel)
if ($OutputPath) { $taskArgs += @('-o', $OutputPath) }
if ($PartialFades) { $taskArgs += '--partial-fades' }
try {
    # The log feeds the progress window without model calls.
    # Windows PowerShell 5 treats stderr as error objects:
    # do not interrupt FFmpeg for ordinary progress messages.
    $ErrorActionPreference = 'Continue'
    & $taskExe @taskArgs 2>&1 | ForEach-Object { $_.ToString() } | Out-File -LiteralPath $taskLog -Encoding UTF8 -Append -Width 4096
    $taskCode = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($taskCode -ne 0) { Add-Content -LiteralPath $taskLog -Value "Error: export exited with code $taskCode" }
} catch {
    Add-Content -LiteralPath $taskLog -Value ('Error: ' + $_.Exception.Message)
    $taskCode = 1
}
Get-Content -LiteralPath $taskLog -Tail 4
Write-Output "Log: $taskLog"
exit $taskCode
