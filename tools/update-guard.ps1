# Repeatable disposable Rust guardian/helper acceptance; never installs NOH.
[CmdletBinding()]
param(
    [string]$VendorDirectory = '.mcp-dev/update-feasibility/vpk/vendor',
    [string]$CompilerDirectory = '.mcp-dev/update-feasibility/llvm-mingw-20260922-ucrt-x86_64'
)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'This qualification runner requires PowerShell 7 on Windows.' }
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$base = Join-Path $projectRoot '.mcp-dev/update-feasibility'
New-Item -ItemType Directory -Path $base -Force | Out-Null
$run = Join-Path $base ('guard-runner-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $run | Out-Null
$profile = Join-Path $run 'profile'
New-Item -ItemType Directory -Path $profile | Out-Null
$previousEnvironment = @{}
foreach ($name in @('CC', 'AR', 'NOH_UPDATE_HELPER', 'NOH_UPDATE_PROBE', 'APPDATA', 'LOCALAPPDATA', 'TEMP', 'TMP')) {
    $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$probeSource = Join-Path $run 'probe.rs'
Set-Content -LiteralPath $probeSource -Encoding utf8 -Value 'fn main() { if std::env::args().nth(1).as_deref() == Some("--hold") { std::thread::sleep(std::time::Duration::from_secs(2)); } }'
Push-Location $projectRoot
try {
    $env:CC = (Resolve-Path -LiteralPath (Join-Path $projectRoot "$CompilerDirectory/bin/clang.exe")).Path
    $env:AR = (Resolve-Path -LiteralPath (Join-Path $projectRoot "$CompilerDirectory/bin/llvm-ar.exe")).Path
    $env:NOH_UPDATE_HELPER = (Resolve-Path -LiteralPath (Join-Path $projectRoot "$VendorDirectory/update_x64.exe")).Path
    $env:NOH_UPDATE_PROBE = Join-Path $run 'probe.exe'
    if ((Get-FileHash -LiteralPath $env:NOH_UPDATE_HELPER -Algorithm SHA256).Hash -ne 'ACEFB4A2CB46CC77ED3C2364DB17F8BD2A25E2197CFEAE56CD85E88A7AC21AC5') {
        throw 'This runner requires the pinned official Velopack 1.2.161 helper.'
    }
    & rustc $probeSource --edition 2021 -C linker=rust-lld -C linker-flavor=ld.lld -o $env:NOH_UPDATE_PROBE *> (Join-Path $run 'compile.log')
    if ($LASTEXITCODE -ne 0) { throw "Fixture compilation failed: $run/compile.log" }
    foreach ($name in @('APPDATA', 'LOCALAPPDATA', 'TEMP', 'TMP')) { Set-Item -LiteralPath "Env:$name" -Value $profile }
    & cargo test --locked --offline --features updates --lib update::windows_guard::tests::authenticated_official_helper_handoff -- --ignored --nocapture *> (Join-Path $run 'acceptance.log')
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) { throw "Guardian acceptance failed ($exitCode): $run/acceptance.log" }
    $match = Select-String -LiteralPath (Join-Path $run 'acceptance.log') -Pattern '^Guardian evidence: (.+)$' | Select-Object -Last 1
    if ($null -eq $match) { throw 'The acceptance process returned no evidence path.' }
    $evidence = [IO.Path]::GetFullPath($match.Matches[0].Groups[1].Value.Trim())
    if (-not $evidence.StartsWith($base + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Evidence escaped the qualification cache.' }
    $result = Get-Content -LiteralPath (Join-Path $evidence 'result.json') -Raw | ConvertFrom-Json
    if (-not ($result.authenticated_helper_apply -and $result.active_lease_refused_before_helper -and $result.direct_active_runtime_refused_without_kill)) { throw 'Required qualification checks are missing.' }
    [ordered]@{ runner = $run; evidence = $evidence; exit_code = $exitCode; qualification = 'Tiny signed Rust/helper fixture; not a real NOH update' } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $run 'result.json') -Encoding utf8
    Get-Content -LiteralPath (Join-Path $run 'result.json')
} finally {
    foreach ($name in $previousEnvironment.Keys) {
        if ($null -eq $previousEnvironment[$name]) {
            Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
        } else {
            Set-Item -LiteralPath "Env:$name" -Value $previousEnvironment[$name]
        }
    }
    Pop-Location
}
