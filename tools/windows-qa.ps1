# Run the existing Windows suite once, recording compilation separately from tests.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Cache,
    [Parameter(Mandatory)][string]$Runtime,
    [switch]$ValidateHelpers
)
. "$PSScriptRoot/update-qualification/common.ps1"
if (-not $IsWindows) { throw 'Windows PowerShell 7 is required' }
$logs = Join-Path $workspace 'logs/setup'
[void][IO.Directory]::CreateDirectory($logs)
$cargo = (Get-Command cargo).Source
$features = 'gui,mcp,updates'
$env:CARGO_TARGET_DIR = Join-Path $workspace 'target'
Write-Json @{features=$features;profile='qa';rustflags=$env:RUSTFLAGS;encoded_rustflags=$env:CARGO_ENCODED_RUSTFLAGS;source_commit=(git -C $workspace rev-parse HEAD).Trim()} "$logs/qa-inputs.json"
Invoke-RecordedProcess (Get-Command python).Source @('tools/windows_test_runtime.py','--cache',$Cache,'--output',$Runtime) $workspace "$logs/test-runtime" 300
$env:NOH_FFMPEG = Join-Path $Runtime 'ffmpeg.exe'
$env:NOH_EXE = "$workspace/target/qa/noh.exe"
$env:NOH_APP_EXE = "$workspace/target/qa/noh-app.exe"
$env:NOH_MCP_EXE = "$workspace/target/qa/noh-mcp.exe"
Invoke-RecordedProcess $cargo @('build','--locked','--offline','--profile','qa','--features',$features,'--bins') $workspace "$logs/workers" 1800
Invoke-RecordedProcess $cargo @('fmt','--all','--check') $workspace "$logs/format" 120
$env:NOH_MEDIA_TESTS = '1'
$tests = @('test','--locked','--offline','--profile','qa','--features',$features,'--lib','--bins','--tests','--example','dev','--example','update-release')
Invoke-RecordedProcess $cargo ($tests + @('--no-run')) $workspace "$logs/compile-tests" 1800
Invoke-RecordedProcess $cargo ($tests + @('--','--test-threads=1')) $workspace "$logs/verification" 1200
if ($ValidateHelpers) {
    # Exercise the same compiler inputs as production without signing, media
    # bundling or release compilation. This mode never populates its Cargo cache.
    Invoke-RecordedProcess $cargo @('build','--locked','--offline','--profile','qa','--features',$features,'--example','update-release') $workspace "$logs/verifier" 300
    $snapshot = Join-Path $workspace 'logs/qa-source-B'
    if (Test-Path -LiteralPath $snapshot) { throw 'Helper validation needs a fresh snapshot' }
    $paths = @(& git -C $workspace -c core.quotepath=false ls-files)
    if ($LASTEXITCODE -ne 0) { throw 'Source enumeration failed' }
    foreach ($relative in $paths) {
        if ($relative -notmatch '^(src/|tools/|tests/|assets/|locales/|docs/|\.cargo/|Cargo\.toml$|Cargo\.lock$|build\.rs$|README\.md$|LICENSE[^/]*$)') { continue }
        $destination = Assert-ChildPath "$snapshot/$relative" $snapshot
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))
        [IO.File]::Copy((Join-Path $workspace $relative), $destination, $false)
    }
    Set-SnapshotVersion $snapshot '0.1.3'
    Invoke-RecordedProcess $cargo @('build','--locked','--offline','--profile','qa','--features',$features,'--example','dev','-vv') $snapshot "$logs/helper-B" 900
    Invoke-RecordedProcess "$workspace/target/qa/examples/dev.exe" @('--help') $snapshot "$logs/helper-help" 15
    Write-Json @{source_root=$snapshot;version='0.1.3';features=$features;profile='qa';rustflags=$env:RUSTFLAGS;release_produced=$false} "$logs/helper-inputs.json"
}
