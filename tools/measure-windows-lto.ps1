# Select a release compiler candidate; this does not qualify a public release.
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Cache, [Parameter(Mandatory)][string]$Runtime)
. "$PSScriptRoot/update-qualification/common.ps1"
if (-not $IsWindows) { throw 'Windows PowerShell 7 is required' }
if ($env:RUSTFLAGS -ne '-C linker-flavor=ld.lld' -or $env:CARGO_ENCODED_RUSTFLAGS) { throw 'Unexpected compiler flags' }
if (@(Get-ChildItem Env:CARGO_PROFILE_*).Count) { throw 'Unexpected inherited Cargo profile overrides' }
$evidence = Join-Path $workspace 'logs/ci-lto'
if (Test-Path -LiteralPath $evidence) { throw 'Fresh evidence directory required' }
[void][IO.Directory]::CreateDirectory($evidence)
$cargo = (Get-Command cargo).Source
$env:CARGO_TARGET_DIR = Join-Path $workspace 'target'
$env:CARGO_INCREMENTAL = '0'
$features = 'gui,mcp,updates'
$names = @('noh','noh-app','noh-mcp','noh-update-guard','noh-update-repair')
$commit = (git rev-parse HEAD).Trim()
$archive = Join-Path $evidence 'source.zip'
& git archive --format=zip "--output=$archive" $commit -- src tools tests assets locales docs .cargo Cargo.toml Cargo.lock build.rs README.md LICENSE
if ($LASTEXITCODE -ne 0) { throw 'Source archive failed' }
$inputs = @{commit=$commit;features=$features;profile='release';rustflags=$env:RUSTFLAGS;rust=(rustc -Vv | Out-String);logical_processors=[Environment]::ProcessorCount;cpu=$env:PROCESSOR_IDENTIFIER;order=@('fat-prime','thin-prime','thin-warm','fat-warm');sources=@{};roots=@{};retries=0;cache_scope='Historical cache read-only; each release dependency graph primed separately; OS cache uncontrolled';production_adopted=$false}
$inputs.source_archive_sha256 = (Get-FileHash -LiteralPath $archive).Hash.ToLowerInvariant()
foreach ($path in @('Cargo.toml','Cargo.lock','.cargo/config.toml','tools/measure-windows-lto.ps1','tools/build_identity.rs','tools/windows_test_runtime.py')) {
    $inputs.sources[$path]=(Get-FileHash -LiteralPath (Join-Path $workspace $path)).Hash.ToLowerInvariant()
}
Write-Json $inputs "$evidence/inputs.json"

function Assert-Disk([long]$Minimum = 12GB) {
    $drive = Get-PSDrive -Name ([IO.Path]::GetPathRoot($evidence).Substring(0,1))
    if ($drive.Free -lt $Minimum) { throw "Insufficient free space: $($drive.Free) bytes" }
}
function Cargo-Phase([string]$Name,[string]$Root,[string[]]$Arguments) {
    Write-Json @{arguments=$Arguments;directory=$Root;lto=$env:CARGO_PROFILE_RELEASE_LTO;rustflags=$env:RUSTFLAGS;incremental=$env:CARGO_INCREMENTAL} "$evidence/$Name.command.json"
    Invoke-RecordedProcess $cargo $Arguments $Root "$evidence/$Name" 1800
}
function Artifacts([string]$Name) {
    $items = [Collections.Generic.List[object]]::new()
    $finished = 0
    foreach ($line in Get-Content "$evidence/$Name.stdout.log") {
        # Cargo -vv forwards build-script stdout with a [package version] prefix
        # even when --message-format=json is requested. Keep it in the raw log.
        if ($line -match '^\[[^\]\r\n]+\] ') { continue }
        $item = $line | ConvertFrom-Json
        if ($item.reason -eq 'compiler-artifact') { $items.Add($item) }
        if ($item.reason -eq 'build-finished') {
            if (-not $item.success) { throw 'Cargo reported a failed build' }
            $finished++
        }
    }
    if ($finished -ne 1) { throw 'One successful build-finished record required' }
    return $items.ToArray()
}
function Hashes([string[]]$Paths) {
    $result = @{}
    foreach ($path in $Paths) { $result[$path]=(Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() }
    return $result
}

$arms = @{}
foreach ($name in $inputs.order) {
    Assert-Disk
    $root = [IO.Path]::GetFullPath("$evidence/source-$name")
    Expand-Archive -LiteralPath $archive -DestinationPath $root
    # Different nonsemantic manifest comments force a fresh NOH workspace build.
    # Versions, application sources, compiler contract and lockfile stay equal.
    [IO.File]::AppendAllText("$root/Cargo.toml", "`n# CI LTO experiment: $name`n")
    $inputs.roots[$name]=@{path=$root;manifest_sha256=(Get-FileHash "$root/Cargo.toml").Hash.ToLowerInvariant();marker="CI LTO experiment: $name"}
    Write-Json $inputs "$evidence/inputs.json"
    $env:CARGO_PROFILE_RELEASE_LTO = if ($name.StartsWith('fat-')) { 'fat' } else { 'thin' }
    $selection = if ($name.EndsWith('-prime')) { '--lib' } else { '--bins' }
    Cargo-Phase $name $root @('build','--locked','--offline','--release','--features',$features,$selection,'-vv','--message-format=json')
    $artifacts = @(Artifacts $name)
    $own = @($artifacts | Where-Object { [IO.Path]::GetFullPath($_.manifest_path) -eq [IO.Path]::GetFullPath("$root/Cargo.toml") })
    $dependencies = @($artifacts | Where-Object { [IO.Path]::GetFullPath($_.manifest_path) -ne [IO.Path]::GetFullPath("$root/Cargo.toml") })
    $recompiled = @($dependencies | Where-Object { -not $_.fresh })
    $dependencyKeys = @($dependencies | ForEach-Object { "$($_.package_id)|$($_.target.kind -join ',')|$($_.target.name)" } | Sort-Object -Unique)
    Write-Json @{dependency_keys=$dependencyKeys;fresh=$dependencies.Count-$recompiled.Count;recompiled=@($recompiled | ForEach-Object { $_.package_id });workspace=@($own | ForEach-Object { @{name=$_.target.name;kind=$_.target.kind;fresh=$_.fresh} })} "$evidence/$name.reuse.json"
    if ($name.EndsWith('-prime')) { continue }
    if ($dependencies.Count -lt 100 -or $recompiled.Count -ne 0) { throw 'Measured dependencies were not all Fresh; comparison refused' }
    $binaries = @($own | Where-Object { $_.target.kind -contains 'bin' -and $_.executable })
    if ($binaries.Count -ne 5 -or @($binaries | Where-Object fresh).Count -or (Compare-Object $names @($binaries.target.name))) { throw 'Expected five freshly compiled NOH executables' }
    $commands = @(Get-Content "$evidence/$name.stderr.log" | Where-Object { $_ -match 'Running.*--crate-name (noh|noh_app|noh_mcp|noh_update_guard|noh_update_repair) .*--crate-type bin ' })
    if ($commands.Count -ne 5) { throw 'Missing final compiler commands' }
    foreach ($line in $commands) {
        $command = $line.Substring($line.IndexOf(' --crate-name '))
        $ltoFlag = if ($env:CARGO_PROFILE_RELEASE_LTO -eq 'thin') { '-C lto=thin ' } else { '-C lto=fat ' }
        foreach ($flag in @($ltoFlag,'-C opt-level=s ','-C codegen-units=1 ','-C strip=symbols ','-C linker=rust-lld ')) {
            if (-not $command.Contains($flag)) { throw "Required compiler flag missing: $flag" }
        }
        if ([regex]::Matches($command,'-C linker-flavor=ld\.lld').Count -ne 1) { throw 'Single LLD flavor flag required' }
    }
    $output = Join-Path $evidence "binaries/$name"
    [void][IO.Directory]::CreateDirectory($output)
    $files = @{}
    $common = $null
    foreach ($binary in $binaries) {
        $path = Join-Path $output "$($binary.target.name).exe"
        Copy-Item -LiteralPath $binary.executable -Destination $path
        Invoke-RecordedProcess $path @('--build-info') $root "$evidence/$name-$($binary.target.name)-identity" 15
        $json = (Get-Content "$evidence/$name-$($binary.target.name)-identity.stdout.log" -Raw).Trim()
        $info = $json | ConvertFrom-Json
        if ($common -and $common -ne $json) { throw 'Worker identities disagree' }
        $common=$json
        if ($info.profile -ne 'release' -or $info.target -ne 'x86_64-pc-windows-gnu' -or (Compare-Object @('default','gui','mcp','updates') @($info.features))) { throw 'Unexpected worker compiler contract' }
        $files[$binary.target.name]=@{path=$path;sha256=(Get-FileHash $path).Hash.ToLowerInvariant();size=(Get-Item $path).Length}
    }
    $arms[$name]=@{process=(Get-Content "$evidence/$name.process.json" -Raw | ConvertFrom-Json);identity=($common | ConvertFrom-Json);files=$files;dependencies=$dependencyKeys}
    Write-Json $arms "$evidence/arms.json"
}
if (Compare-Object $arms['thin-warm'].dependencies $arms['fat-warm'].dependencies) { throw 'Measured dependency sets differ' }
if ($arms['thin-warm'].identity.options_sha256 -eq $arms['fat-warm'].identity.options_sha256) { throw 'LTO override absent from build identity' }
$old = $arms['fat-warm'].process.seconds
$new = $arms['thin-warm'].process.seconds
$saved = $old-$new
$fraction = $saved/$old
$selected = $saved -ge 120 -and $fraction -ge 0.20
$comparison = @{baseline_seconds=$old;candidate_seconds=$new;saved_seconds=$saved;saved_fraction=$fraction;candidate_selected=$selected;production_adopted=$false;required_passed=$null;required_ignored=$null;scope='Matched warm NOH release compilation including five executable links; no final native qualification';order=$inputs.order;next_gate='Final-byte startup/import/media/preview/export/update-repair acceptance and size/performance comparison required before adoption'}
Write-Json $comparison "$evidence/comparison.json"
if (-not $selected) { Write-Output 'Compiler candidate rejected by the predeclared timing threshold; production unchanged'; return }
if ($env:GITHUB_OUTPUT) { 'candidate_selected=true' >> $env:GITHUB_OUTPUT }

# Test the actual selected workers. Retain copies independently of Cargo outputs.
$root = $inputs.roots['thin-warm'].path
$env:CARGO_PROFILE_RELEASE_LTO = 'thin'
$env:NOH_MEDIA_TESTS = '1'
$env:NOH_FFMPEG = Join-Path $Runtime 'ffmpeg.exe'
$env:NOH_EXE = $arms['thin-warm'].files['noh'].path
$env:NOH_APP_EXE = $arms['thin-warm'].files['noh-app'].path
$env:NOH_MCP_EXE = $arms['thin-warm'].files['noh-mcp'].path
Invoke-RecordedProcess (Get-Command python).Source @('tools/windows_test_runtime.py','--cache',$Cache,'--output',$Runtime) $root "$evidence/test-runtime" 300
Assert-Disk 6GB
$testArgs = @('test','--locked','--offline','--release','--features',$features,'--lib','--bins','--tests','--example','dev','--example','update-release','--no-run','--message-format=json')
Cargo-Phase 'compile-tests' $root $testArgs
$tests = @(Artifacts 'compile-tests' | Where-Object { $_.profile.test -and $_.executable })
Cargo-Phase 'metadata' $root @('metadata','--locked','--offline','--no-deps','--format-version','1','--features',$features)
$metadata = Get-Content "$evidence/metadata.stdout.log" -Raw | ConvertFrom-Json
$package = @($metadata.packages | Where-Object name -eq 'noh')
if ($package.Count -ne 1) { throw 'Unique workspace package required' }
$expected = @($package[0].targets | Where-Object { $_.kind[0] -in @('lib','bin','test') -or ($_.kind[0] -eq 'example' -and $_.name -in @('dev','update-release')) } | ForEach-Object { "$($_.kind[0])-$($_.name)" } | Sort-Object)
$actual = @($tests | ForEach-Object { "$($_.target.kind[0])-$($_.target.name)" } | Sort-Object)
if ((Compare-Object $expected $actual) -or $actual.Count -ne @($actual | Sort-Object -Unique).Count) { throw 'Test target inventory differs from Cargo metadata' }
$owned = @($tests.executable) + @($arms['thin-warm'].files.Values.path) + @($arms['fat-warm'].files.Values.path) + @($env:NOH_FFMPEG)
$before = Hashes $owned
Write-Json $before "$evidence/test-inputs.json"
$inventory = @{}
$passed=0; $ignored=0
$deadline = [DateTime]::UtcNow.AddSeconds(1200)
foreach ($test in $tests) {
    $label = "$($test.target.kind[0])-$($test.target.name)"
    $remaining = [int][Math]::Ceiling(($deadline-[DateTime]::UtcNow).TotalSeconds)
    if ($remaining -le 0) { throw 'Suite execution deadline exceeded' }
    Invoke-RecordedProcess $test.executable @('--list') $root "$evidence/list-$label" ([Math]::Min(60,$remaining))
    $discovered = @(Get-Content "$evidence/list-$label.stdout.log" | Where-Object { $_ -match ': test$' } | ForEach-Object { $_ -replace ': test$','' } | Sort-Object)
    $remaining = [int][Math]::Ceiling(($deadline-[DateTime]::UtcNow).TotalSeconds)
    if ($remaining -le 0) { throw 'Suite execution deadline exceeded' }
    Invoke-RecordedProcess $test.executable @('--test-threads=1') $root "$evidence/test-$label" ([Math]::Min(600,$remaining))
    $log = Get-Content "$evidence/test-$label.stdout.log" -Raw
    $summary = [regex]::Match($log,'test result: ok\. (\d+) passed; 0 failed; (\d+) ignored; 0 measured; 0 filtered out')
    if (-not $summary.Success) { throw "Missing successful complete test summary: $label" }
    $results = @([regex]::Matches($log,'(?m)^test (.+) \.\.\. (?:ok|ignored[^\r\n]*)\r?$') | ForEach-Object { $_.Groups[1].Value } | Sort-Object)
    if ($discovered.Count -ne $results.Count -or ($discovered.Count -and (Compare-Object $discovered $results))) { throw "Test-name inventory mismatch: $label" }
    $passed += [int]$summary.Groups[1].Value; $ignored += [int]$summary.Groups[2].Value
    $inventory[$label]=@{names=$discovered;passed=[int]$summary.Groups[1].Value;ignored=[int]$summary.Groups[2].Value}
    Write-Json $inventory "$evidence/test-inventory.json"
}
if ($passed -ne 566 -or $ignored -ne 26) { throw "Required suite inventory differs: $passed passed, $ignored ignored" }
$after = Hashes $owned
foreach ($path in $before.Keys) { if ($before[$path] -ne $after[$path]) { throw 'Measured executable changed during tests' } }
$remainingProcesses = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -in $owned } | Select-Object Id,ProcessName,Path)
Write-Json @{remaining=$remainingProcesses;hashes=$after} "$evidence/after-tests.json"
if ($remainingProcesses.Count) { throw 'Owned workload remains after tests' }
$comparison.required_passed=$passed; $comparison.required_ignored=$ignored
Write-Json $comparison "$evidence/comparison.json"
Get-Content "$evidence/comparison.json"
