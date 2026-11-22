# Run the caption effect tests that need OpenGL 3.3, with Mesa's software renderer.
# They are ignored by default, so tools/windows-qa.ps1 compiles them without running
# them. Run this after it, on a runner that builds no package: Mesa is test material.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Cache,
    [Parameter(Mandatory)][string]$Runtime
)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Windows PowerShell 7 is required' }
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$names = @(
    'gpu_checks',
    'a_chosen_effect_burns_and_warns_when_its_installed_font_is_missing',
    'a_cue_too_tall_for_libass_is_drawn_by_classique_with_an_overflow_warning',
    'a_missing_installed_font_is_drawn_in_the_plugin_face_with_a_warning',
    'a_chosen_effect_draws_the_short_and_warns_when_its_installed_font_is_missing',
    'scene_jobs_check_look_burn_clip_and_refuse'
)
# The same build as tools/windows-qa.ps1, so nothing is compiled again.
$env:CARGO_TARGET_DIR = Join-Path $workspace 'target'
$env:NOH_FFMPEG = Join-Path $Runtime 'ffmpeg.exe'
$env:NOH_EXE = "$workspace/target/qa/noh.exe"
$env:NOH_APP_EXE = "$workspace/target/qa/noh-app.exe"
$env:NOH_MCP_EXE = "$workspace/target/qa/noh-mcp.exe"
$env:NOH_MEDIA_TESTS = '1'
$env:GALLIUM_DRIVER = 'llvmpipe'
$env:PATH = "$Runtime;$env:PATH"
if (-not (Test-Path -LiteralPath $env:NOH_FFMPEG -PathType Leaf)) { throw 'Run tools/windows-qa.ps1 first: its FFmpeg is missing' }
# The tests draw inside their own process; a worker they start draws beside noh.exe.
$folders = @((Join-Path $workspace 'target/qa/deps'), (Join-Path $workspace 'target/qa'))
$logs = Join-Path $workspace 'logs/setup'
[void][IO.Directory]::CreateDirectory($logs)
$log = Join-Path $logs 'effect-opengl-tests.log'
$python = (Get-Command python).Source
& $python (Join-Path $PSScriptRoot 'software_opengl.py') deploy --cache $Cache @folders
if ($LASTEXITCODE -ne 0) { throw 'Software OpenGL could not be deployed' }
$failure = $null
try {
    $arguments = @('test', '--locked', '--offline', '--profile', 'qa', '--features', 'gui,mcp,motion',
        '--lib', '--bins', '--tests', '--', '--ignored', '--test-threads=1') + $names
    & cargo @arguments *> $log
    $exit = $LASTEXITCODE
    Get-Content -LiteralPath $log | Select-String -Pattern '^test .* \.\.\. |^test result:|panicked|^error' | ForEach-Object Line
    if ($exit -ne 0) { throw "The effect tests failed with exit code $exit" }
    # A filter that matches nothing also exits 0: require each test to have run and passed once.
    $lines = Get-Content -LiteralPath $log
    foreach ($name in $names) {
        $pattern = '^test ([a-z_0-9]+::)*' + [regex]::Escape($name) + ' \.\.\. ok$'
        $count = @($lines | Where-Object { $_ -match $pattern }).Count
        if ($count -ne 1) { throw "Expected one passing run of $name, found $count" }
    }
} catch {
    $failure = $_
}
# The job caches target/: a Mesa file left there would break every later run.
& $python (Join-Path $PSScriptRoot 'software_opengl.py') remove --cache $Cache @folders
$removed = $LASTEXITCODE -eq 0
if ($failure) {
    if (-not $removed) { Write-Warning 'Software OpenGL files were not removed' }
    throw $failure
}
if (-not $removed) { throw 'Software OpenGL files were not removed' }
