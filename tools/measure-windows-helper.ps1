# Compare snapshot helper recompilation with one reusable explicit-root helper.
[CmdletBinding()]
param()
. "$PSScriptRoot/update-qualification/common.ps1"
if (-not $IsWindows) { throw 'Windows PowerShell 7 is required' }
$evidence = Join-Path $workspace 'logs/ci-helper'
if (Test-Path -LiteralPath $evidence) { throw 'Fresh evidence directory required' }
[void][IO.Directory]::CreateDirectory($evidence)
$baseline = 'bcf286a02d4e730fbaec7fd09be770762f78bd72'
$snapshot = "$evidence/source-B"
$archive = "$evidence/source.zip"
& git archive --format=zip "--output=$archive" $baseline -- src tools tests assets locales docs .cargo Cargo.toml Cargo.lock build.rs README.md LICENSE
if ($LASTEXITCODE -ne 0) { throw 'Baseline source archive failed' }
Expand-Archive -LiteralPath $archive -DestinationPath $snapshot
Set-SnapshotVersion $snapshot '0.1.3'
$cargo = (Get-Command cargo).Source
$env:CARGO_TARGET_DIR = Join-Path $workspace 'target'
$features = 'gui,mcp,updates'
# Both actual pipelines already require these workers; keep this shared cost
# outside the helper comparison. No application release or bundle is produced.
Invoke-RecordedProcess $cargo @('build','--locked','--offline','--profile','qa','--features',$features,'--bins') $workspace "$evidence/workers" 1800
$helperArgs = @('build','--locked','--offline','--profile','qa','--features',$features,'--example','dev','-vv')
Invoke-RecordedProcess $cargo $helperArgs $workspace "$evidence/reusable-helper" 900
$helper = "$evidence/reusable-dev.exe"
Copy-Item -LiteralPath "$workspace/target/qa/examples/dev.exe" -Destination $helper
$helperHash = (Get-FileHash -LiteralPath $helper).Hash.ToLowerInvariant()
Invoke-RecordedProcess $cargo $helperArgs $snapshot "$evidence/snapshot-helper" 900
Invoke-RecordedProcess "$workspace/target/qa/examples/dev.exe" @('--help') $snapshot "$evidence/snapshot-help" 15
$workerHashes = @{}
foreach ($name in @('noh','noh-app','noh-mcp','noh-update-guard','noh-update-repair')) {
    $workerHashes[$name] = (Get-FileHash -LiteralPath "$workspace/target/qa/$name.exe").Hash.ToLowerInvariant()
}
$inputs = @{commit=(git rev-parse HEAD).Trim();baseline=$baseline;snapshot=$snapshot;version='0.1.3';features=$features;profile='qa';rustflags=$env:RUSTFLAGS;rust=(rustc -Vv | Out-String);helper_sha256=$helperHash;workers=$workerHashes;order=@('reusable-helper','snapshot-helper');sources=@{}}
foreach ($name in @('tools/dev.rs','tools/update-setup-clients.ps1','tools/measure-windows-helper.ps1','tools/package.rs','Cargo.toml','Cargo.lock')) {
    $inputs.sources[$name]=(Get-FileHash -LiteralPath "$workspace/$name").Hash.ToLowerInvariant()
}
Write-Json $inputs "$evidence/inputs.json"
# Actual tiny Cargo projects distinguish explicit source selection from the
# compiled-in root, including spaces, Unicode and a changed package version.
$env:CARGO_TARGET_DIR = "$evidence/synthetic-target"
$cases = @()
foreach ($version in @('0.8.7','0.8.8')) {
    $root = [IO.Path]::GetFullPath("$evidence/source $version Δ")
    [void][IO.Directory]::CreateDirectory($root)
    [IO.File]::WriteAllText("$root/Cargo.toml", "[package]`nname = `"noh`"`nversion = `"$version`"`nedition = `"2024`"`n[workspace]`n[[bin]]`nname = `"noh`"`npath = `"main.rs`"`n")
    [IO.File]::WriteAllText("$root/main.rs", 'fn main() { println!("{}|{}", env!("CARGO_PKG_VERSION"), env!("CARGO_MANIFEST_DIR")); }')
    Invoke-RecordedProcess $cargo @('generate-lockfile','--offline','--manifest-path',"$root/Cargo.toml") $workspace "$evidence/lock-$version" 60
    Invoke-RecordedProcess $helper @('build','--source-root',$root,'--no-bundle','--offline') $workspace "$evidence/build-$version" 120
    Invoke-RecordedProcess "$env:CARGO_TARGET_DIR/release/noh.exe" @() $workspace "$evidence/run-$version" 15
    $output = (Get-Content "$evidence/run-$version.stdout.log" -Raw).Trim()
    if ($output -ne "$version|$root") { throw "Wrong source built: $output" }
    $cases += @{version=$version;root=$root;output=$output;sha256=(Get-FileHash "$env:CARGO_TARGET_DIR/release/noh.exe").Hash.ToLowerInvariant()}
}
$invalid = "$evidence/invalid-root"
[void][IO.Directory]::CreateDirectory($invalid)
try { Invoke-RecordedProcess $helper @('build','--source-root',$invalid,'--no-bundle','--offline') $workspace "$evidence/invalid" 15 } catch {
    $result = Get-Content "$evidence/invalid.process.json" -Raw | ConvertFrom-Json
    if ($result.exit -ne 1 -or $result.timed_out -or (Get-Content "$evidence/invalid.stderr.log" -Raw) -notmatch 'Build source root must contain Cargo.toml') { throw }
}
$negative = Get-Content "$evidence/invalid.process.json" -Raw | ConvertFrom-Json
if ($negative.exit -ne 1 -or (Get-FileHash -LiteralPath $helper).Hash.ToLowerInvariant() -ne $helperHash) { throw 'Helper identity or invalid-root result changed' }
$env:CARGO_TARGET_DIR = Join-Path $workspace 'target'
Invoke-RecordedProcess $cargo @('test','--locked','--offline','--profile','qa','--features',$features,'--example','dev','--','--test-threads=1') $workspace "$evidence/helper-tests" 900
foreach ($name in $workerHashes.Keys) { if ((Get-FileHash -LiteralPath "$workspace/target/qa/$name.exe").Hash.ToLowerInvariant() -ne $workerHashes[$name]) { throw 'Application worker changed during helper validation' } }
$old = Get-Content "$evidence/snapshot-helper.process.json" -Raw | ConvertFrom-Json
$new = Get-Content "$evidence/reusable-helper.process.json" -Raw | ConvertFrom-Json
Write-Json @{scope='Hosted helper compilation after QA workers; no whole-release or bundle timing';baseline_seconds=$old.seconds;candidate_seconds=$new.seconds;saved_seconds=($old.seconds-$new.seconds);cases=$cases;invalid_root_exit=$negative.exit;helper_sha256=$helperHash;order=$inputs.order;os_cache='Uncontrolled; reusable helper runs first';release_produced=$false} "$evidence/comparison.json"
Get-Content "$evidence/comparison.json"
