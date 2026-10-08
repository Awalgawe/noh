# Disposable CI comparison; no production test scheduling is changed here.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Cache,
    [Parameter(Mandatory)][string]$Runtime
)
. "$PSScriptRoot/update-qualification/common.ps1"
if (-not $IsWindows) { throw 'Windows PowerShell 7 is required' }
$evidence = Join-Path $workspace 'logs/ci-media'
if (Test-Path -LiteralPath $evidence) { throw 'Fresh evidence directory required' }
[void][IO.Directory]::CreateDirectory($evidence)
$baseline = 'a8ced2028401a28e4fddd655395335ebedfdbaf8'
$generated = Join-Path $workspace 'tests/ci_media_baseline.rs'
if (Test-Path -LiteralPath $generated) { throw 'Baseline test target already exists' }
# Build both test modules in one graph against the same current library/workers.
# The extra target is diagnostic input, never a claimed release source identity.
& git diff --quiet $baseline -- tests/media.rs tests/media/smart_copy.rs tests/media/verification.rs tests/support/mod.rs tools/process.rs src Cargo.toml Cargo.lock .cargo build.rs
if ($LASTEXITCODE -ne 0) { throw 'Uncontrolled application or media support change' }
$original = & git show "${baseline}:tests/media/cli.rs"
if ($LASTEXITCODE -ne 0) { throw 'Cannot read original fixture implementation' }
[IO.File]::WriteAllText("$evidence/original-cli.rs", ($original -join "`n") + "`n")
$entry = [IO.File]::ReadAllText("$workspace/tests/media.rs")
[IO.File]::WriteAllText($generated, $entry.Replace('media/cli.rs','../logs/ci-media/original-cli.rs'))
$cargo = (Get-Command cargo).Source
$env:CARGO_TARGET_DIR = Join-Path $workspace 'target'
$env:NOH_MEDIA_TESTS = '1'
$env:NOH_FFMPEG = Join-Path $Runtime 'ffmpeg.exe'
$env:NOH_EXE = "$workspace/target/qa/noh.exe"
$env:NOH_APP_EXE = "$workspace/target/qa/noh-app.exe"
$env:NOH_MCP_EXE = "$workspace/target/qa/noh-mcp.exe"
Invoke-RecordedProcess (Get-Command python).Source @('tools/windows_test_runtime.py','--cache',$Cache,'--output',$Runtime) $workspace "$evidence/test-runtime" 300
$features = 'gui,mcp,updates'
Invoke-RecordedProcess $cargo @('build','--locked','--offline','--profile','qa','--features',$features,'--bins') $workspace "$evidence/workers" 1800
$tests = @('test','--locked','--offline','--profile','qa','--features',$features,'--lib','--bins','--tests','--example','dev','--example','update-release')
Invoke-RecordedProcess $cargo ($tests + @('--no-run','--message-format=json')) $workspace "$evidence/compile" 1800
$artifacts = @(Get-Content "$evidence/compile.stdout.log" | ForEach-Object {
    $item = $_ | ConvertFrom-Json
    if ($item.reason -eq 'compiler-artifact' -and $item.profile.test -and $item.executable) { $item }
})
$originalArtifacts = @($artifacts | Where-Object { $_.target.name -eq 'ci_media_baseline' })
$candidateArtifacts = @($artifacts | Where-Object { $_.target.name -eq 'media' })
if ($originalArtifacts.Count -ne 1 -or $candidateArtifacts.Count -ne 1) { throw 'Unique original and candidate media harnesses required' }
$originalExe = $originalArtifacts[0].executable
$candidateExe = $candidateArtifacts[0].executable
$owned = @($originalExe,$candidateExe,$env:NOH_EXE,$env:NOH_APP_EXE,$env:NOH_MCP_EXE,$env:NOH_FFMPEG)
$inputs = @{commit=(git rev-parse HEAD).Trim();baseline=$baseline;features=$features;profile='qa';rustflags=$env:RUSTFLAGS;rust=(rustc -Vv | Out-String);logical_processors=[Environment]::ProcessorCount;order=@('original-serial','candidate-serial','candidate-two');retries=0;files=@{};sources=@{};scope='Windows media harness; generated baseline module shares current library and workers; no release qualification'}
foreach ($path in $owned) { $inputs.files[$path]=(Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() }
foreach ($path in @('tests/media/cli.rs','tests/media.rs','tests/support/mod.rs','tests/ci_media_baseline.rs','logs/ci-media/original-cli.rs','tools/measure-windows-media.ps1','tools/windows_test_runtime.py','assets/windows-native.lock.json','Cargo.toml','Cargo.lock')) {
    $inputs.sources[$path]=(Get-FileHash -LiteralPath (Join-Path $workspace $path)).Hash.ToLowerInvariant()
}
Write-Json $inputs "$evidence/inputs.json"
foreach ($pair in @(@('original',$originalExe),@('candidate',$candidateExe))) {
    Invoke-RecordedProcess $pair[1] @('--list') $workspace "$evidence/$($pair[0])-list" 60
}
$names = @(Get-Content "$evidence/original-list.stdout.log" | Where-Object { $_ -match ': test$' } | Sort-Object)
$candidateNames = @(Get-Content "$evidence/candidate-list.stdout.log" | Where-Object { $_ -match ': test$' } | Sort-Object)
if ($names.Count -ne 14 -or (Compare-Object $names $candidateNames)) { throw 'Media test inventory mismatch' }
$results = @{}
foreach ($arm in @(@('original-serial',$originalExe,1),@('candidate-serial',$candidateExe,1),@('candidate-two',$candidateExe,2))) {
    $name,$exe,$threads = $arm
    $prefix = "$evidence/$name"
    if (@(Get-Process -Name 'media-*','ci_media_baseline-*','noh','ffmpeg' -ErrorAction SilentlyContinue | Where-Object { $_.Path -in $owned }).Count) { throw 'Owned workload already running' }
    $stop = "$prefix.stop"
    $sampler = Start-ThreadJob -ArgumentList $owned,$stop,"$prefix.resources.json" -ScriptBlock {
        param($Paths,$Stop,$Output)
        $stats = @{samples=0;peak_processes=0;peak_ffmpeg=0;peak_working_set_bytes=0;observed=@{};unavailable=0;scope='Sampled exact executable paths; short-lived processes may be missed'}
        while (-not (Test-Path -LiteralPath $Stop)) {
            $processes = @(Get-Process -Name 'media-*','ci_media_baseline-*','noh','ffmpeg' -ErrorAction SilentlyContinue | Where-Object { $_.Path -in $Paths })
            $bytes=0
            foreach ($process in $processes) {
                try {
                    $start=$process.StartTime
                    if ($null -eq $start) { $stats.unavailable++; continue }
                    $bytes += $process.WorkingSet64
                    $stats.observed["$($process.Id):$($start.ToUniversalTime().Ticks)"] = @{name=$process.ProcessName;cpu_seconds=$process.CPU}
                } catch { $stats.unavailable++ }
            }
            $stats.samples++
            $stats.peak_processes=[Math]::Max($stats.peak_processes,$processes.Count)
            $stats.peak_ffmpeg=[Math]::Max($stats.peak_ffmpeg,@($processes | Where-Object ProcessName -eq 'ffmpeg').Count)
            $stats.peak_working_set_bytes=[Math]::Max($stats.peak_working_set_bytes,$bytes)
            Start-Sleep -Milliseconds 500
        }
        $stats | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $Output
    }
    try {
        Invoke-RecordedProcess $exe @("--test-threads=$threads") $workspace $prefix 600
    } finally {
        [IO.File]::WriteAllText($stop,'completed')
        if (-not (Wait-Job $sampler -Timeout 5)) { Stop-Job $sampler; throw 'Sampler failed to stop' }
        Receive-Job $sampler -ErrorAction Stop
        Remove-Job $sampler
    }
    $left = @(Get-Process -Name 'media-*','ci_media_baseline-*','noh','ffmpeg' -ErrorAction SilentlyContinue | Where-Object { $_.Path -in $owned })
    Write-Json @{remaining=@($left | Select-Object Id,ProcessName,Path)} "$prefix.remaining.json"
    if ($left.Count) { throw 'Owned process remains after test completion' }
    $log = Get-Content "$prefix.stdout.log" -Raw
    if ($log -notmatch 'test result: ok\. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out') { throw 'Incomplete media execution' }
    $passed = @([regex]::Matches($log,'(?m)^test (.+) \.\.\. ok\r?$') | ForEach-Object { $_.Groups[1].Value + ': test' } | Sort-Object)
    if (Compare-Object $names $passed) { throw 'Executed test names differ from discovery' }
    $results[$name] = Get-Content "$prefix.process.json" -Raw | ConvertFrom-Json
}
# Execute every remaining original target with the unchanged serial policy.
# Explicit target selection avoids rerunning either media diagnostic here.
$otherTests = @($artifacts | Where-Object { $_.target.kind -contains 'test' -and $_.target.name -notin @('media','ci_media_baseline') } | ForEach-Object { $_.target.name } | Sort-Object -Unique)
$remaining = @('test','--locked','--offline','--profile','qa','--features',$features,'--lib','--bins','--example','dev','--example','update-release')
foreach ($target in $otherTests) { $remaining += @('--test',$target) }
Invoke-RecordedProcess $cargo ($remaining + @('--','--test-threads=1')) $workspace "$evidence/remaining-suite" 1200
$summaries=[regex]::Matches((Get-Content "$evidence/remaining-suite.stdout.log" -Raw),'test result: ok\. (\d+) passed; 0 failed; (\d+) ignored; 0 measured; 0 filtered out')
$passed=14; $ignored=0
foreach ($summary in $summaries) { $passed += [int]$summary.Groups[1].Value; $ignored += [int]$summary.Groups[2].Value }
if ($passed -ne 565 -or $ignored -ne 26) { throw "Required suite inventory differs: $passed passed, $ignored ignored" }
if ((Get-Content "$evidence/remaining-suite.stderr.log" -Raw) -match 'Compiling ') { throw 'Unexpected recompilation after measurement' }
foreach ($path in $owned) { if ((Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -ne $inputs.files[$path]) { throw 'Measured executable changed' } }
Write-Json @{scope=$inputs.scope;results=$results;equal_media_names=$true;required_passed=$passed;required_ignored=$ignored;fixture_saved_seconds=($results['original-serial'].seconds-$results['candidate-serial'].seconds);scheduling_saved_seconds=($results['candidate-serial'].seconds-$results['candidate-two'].seconds);combined_saved_seconds=($results['original-serial'].seconds-$results['candidate-two'].seconds);scheduling_adopted=$false;order_caches='Original first; raw inputs cached; OS cache uncontrolled'} "$evidence/comparison.json"
Get-Content "$evidence/comparison.json"
