# Focused orchestration regressions; no build, package installation or network.
param()
. "$PSScriptRoot/update-qualification/common.ps1"
# The entry-point regressions share the runner's lock; do not race a native qualification.
if (Test-Path "$qualificationBase/qualification.lock") {
    $probe = [IO.File]::Open("$qualificationBase/qualification.lock", 'Open', 'ReadWrite', 'None')
    $probe.Dispose()
}
$testRoot = Join-Path $workspace ('.mcp-dev/qualification-tests-' + [guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($testRoot)
$pwsh = (Get-Command pwsh).Source
$checks = [Collections.Generic.List[string]]::new()
function Check([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "FAILED: $Message" }
    $checks.Add($Message)
}
function Refused([scriptblock]$Action, [string]$Message) {
    $refused = $false
    try { & $Action | Out-Null } catch { $refused = $true }
    Check $refused $Message
}
try {
    Refused { Assert-ChildPath "$testRoot/../escape" $testRoot } 'Parent traversal refused'
    Refused { Assert-ChildPath "$testRoot-sibling/file" $testRoot } 'Sibling prefix refused'
    Refused { Assert-ChildPath $testRoot $testRoot } 'Root itself refused'
    [void][IO.Directory]::CreateDirectory("$testRoot/outside")
    [void](New-Item -ItemType Junction -Path "$testRoot/junction" -Target "$testRoot/outside")
    Refused { Assert-PlainPath "$testRoot/junction/missing-child" } 'Junction ancestor refused before file creation'
    Assert-VersionPair '0.6.1' '0.6.2'
    Refused { Assert-VersionPair '0.6.2' '0.6.1' } 'Downgrade refused'
    Refused { Assert-VersionPair '0.6.1' '0.6.1' } 'Equal versions refused'
    Refused { Assert-VersionPair '01.2.3' '2.0.0' } 'Ambiguous version refused'
    [void][IO.Directory]::CreateDirectory("$testRoot/source-A")
    [void][IO.Directory]::CreateDirectory("$testRoot/source-B")
    $toml = "[package]`nname = `"noh`"`nversion = `"0.1.0`"`n"
    $lock = "[[package]]`nname = `"noh`"`nversion = `"0.1.0`"`n`n[[package]]`nname = `"other`"`nversion = `"9.9.9`"`n"
    foreach ($variant in @('A','B')) {
        [IO.File]::WriteAllText("$testRoot/source-$variant/Cargo.toml", $toml)
        [IO.File]::WriteAllText("$testRoot/source-$variant/Cargo.lock", $lock)
    }
    Set-SnapshotVersion "$testRoot/source-A" '0.6.1'
    Set-SnapshotVersion "$testRoot/source-B" '0.6.2'
    Check ((Get-Content "$testRoot/source-B/Cargo.lock" -Raw).Contains('version = "9.9.9"')) 'Dependency versions preserved'
    Check ((Get-Content "$testRoot/source-A/Cargo.toml" -Raw).Contains('version = "0.6.1"')) 'A version patched'
    Check ((Get-Content "$testRoot/source-B/Cargo.toml" -Raw).Contains('version = "0.6.2"')) 'B version patched'
    $inventory = @(foreach ($file in @('Cargo.toml','Cargo.lock')) { @{path=$file;sha256_A=(Get-FileHash "$testRoot/source-A/$file").Hash;sha256_B=(Get-FileHash "$testRoot/source-B/$file").Hash} })
    Write-Json $inventory "$testRoot/source-inventory.json"
    Assert-Snapshot $testRoot 'A'
    'unreviewed' | Set-Content "$testRoot/source-B/extra.rs"
    Refused { Assert-Snapshot $testRoot 'B' } 'Added frozen source refused'
    Add-Content "$testRoot/source-A/Cargo.toml" '# changed'
    Refused { Assert-Snapshot $testRoot 'A' } 'Modified frozen source refused'
    $harnessInventory = @(foreach ($file in @('update-qualification.ps1','update-qualification/common.ps1','update-qualification/prepare.ps1','update-qualification/build.ps1','update-qualification/package.ps1','update-qualification/accept.ps1','update-repair-qualification.ps1')) {
        $destination = "$testRoot/current/tools/$file"
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))
        'original' | Set-Content $destination
        @{path="tools/$file";sha256_source=(Get-FileHash $destination).Hash}
    })
    Write-Json $harnessInventory "$testRoot/source-inventory.json"
    Assert-Harness $testRoot "$testRoot/current"
    'changed' | Add-Content "$testRoot/current/tools/update-qualification/accept.ps1"
    Refused { Assert-Harness $testRoot "$testRoot/current" } 'Changed continuation harness refused'
    $child = "$testRoot/child with spaces.ps1"
    'param([string]$Value); [Console]::WriteLine($Value); [Console]::Error.WriteLine("expected stderr"); exit 37' | Set-Content $child
    Refused { Invoke-RecordedProcess -File $pwsh -Arguments @('-NoProfile','-File',$child,'two words & literal') -Directory $testRoot -Log "$testRoot/failure" } 'Child failure propagates'
    $failure = Get-Content "$testRoot/failure.process.json" -Raw | ConvertFrom-Json
    Check ($failure.exit -eq 37 -and $failure.seconds -gt 0) 'Exact child exit and duration recorded'
    Check ((Get-Content "$testRoot/failure.stdout.log" -Raw).Trim() -eq 'two words & literal') 'Arguments passed literally with spaces'
    Check ((Get-Content "$testRoot/failure.stderr.log" -Raw).Trim() -eq 'expected stderr') 'Error output retained'
    Refused { Invoke-RecordedProcess -File $pwsh -Arguments @('-NoProfile','-Command','[Console]::WriteLine("before timeout"); Start-Sleep -Seconds 30') -Directory $testRoot -Log "$testRoot/timeout" -TimeoutSeconds 2 } 'Child deadline enforced'
    $timeout = Get-Content "$testRoot/timeout.process.json" -Raw | ConvertFrom-Json
    Check ($timeout.timed_out -and -not (Get-Process -Id $timeout.pid -ErrorAction SilentlyContinue)) 'Timed-out owned process stopped and recorded'
    Check ((Get-Content "$testRoot/timeout.stdout.log" -Raw).Trim() -eq 'before timeout') 'Partial output survives timeout'
    Refused { Invoke-RecordedProcess -File $pwsh -Arguments @('-NoProfile','-Command','Start-Sleep -Seconds 30') -Directory $testRoot -Log "$testRoot/preserved" -TimeoutSeconds 1 -PreserveOnTimeout -OwnerMarker "$testRoot/owner.json" } 'Preserved acceptance deadline recorded'
    $owner = Get-Content "$testRoot/owner.json" -Raw | ConvertFrom-Json
    try { Refused { Assert-NoRecordedOwner "$testRoot/owner.json" } 'Continuation refused while preserved acceptance is alive' }
    finally { $owned = Get-Process -Id $owner.pid; $owned.Kill($true); [void]$owned.WaitForExit(5000) }
    Assert-NoRecordedOwner "$testRoot/owner.json"
    # Exercise the real entry point's failure terminal, without compiling or touching runtime inputs.
    $before = @(Get-ChildItem $qualificationBase -Filter 'attempt-*-result.json' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName)
    $casePath = Join-Path $qualificationBase ('test-missing-' + [guid]::NewGuid().ToString('N'))
    Refused { Invoke-RecordedProcess -File $pwsh -Arguments @('-NoProfile','-File',"$workspace/tools/update-qualification.ps1",'-Stage','All','-Case',$casePath,'-RuntimeDirectory',"$testRoot/missing-runtime") -Directory $workspace -Log "$testRoot/entry-failure" } 'Entry stops on failed prerequisite'
    $new = @(Get-ChildItem $qualificationBase -Filter 'attempt-*-result.json' | Where-Object FullName -NotIn $before)
    Check ($new.Count -eq 1) 'One terminal report on preflight exception'
    $terminal = Get-Content $new[0].FullName -Raw | ConvertFrom-Json
    Check ($terminal.status -eq 'failed' -and $terminal.steps[0].status -eq 'failed') 'Failure terminal is explicit'
    Check (@($terminal.steps | Select-Object -Skip 1 | Where-Object status -NE 'skipped').Count -eq 0) 'Dependent stages skipped after failure'
    Check (-not (Test-Path $casePath)) 'Failed preflight creates no case'
    Refused { Invoke-RecordedProcess -File $pwsh -Arguments @('-NoProfile','-File',"$workspace/tools/update-qualification.ps1",'-Stage','Prepare','-Case',$testRoot) -Directory $workspace -Log "$testRoot/outside" } 'Entry refuses an unconfined case'
    $collision = Join-Path $qualificationBase ('test-collision-' + [guid]::NewGuid().ToString('N'))
    [void][IO.Directory]::CreateDirectory($collision)
    'preserve' | Set-Content "$collision/sentinel"
    Refused { Invoke-RecordedProcess -File $pwsh -Arguments @('-NoProfile','-File',"$workspace/tools/update-qualification.ps1",'-Stage','Prepare','-Case',$collision) -Directory $workspace -Log "$testRoot/collision" } 'Entry refuses an existing case'
    Check ((Get-Content "$collision/sentinel") -eq 'preserve') 'Existing case preserved'
    Write-Json @{passed=$true;checks=@($checks);native_execution_performed=$false} "$testRoot/result.json"
    Write-Output "$($checks.Count) checks passed: $testRoot/result.json"
} catch {
    Write-Json @{passed=$false;checks=@($checks);error=$_.Exception.Message} "$testRoot/result.json"
    throw
}
