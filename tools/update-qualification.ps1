# Repository entry point. Preflight is read-only; All performs a disposable local installation.
# Requires PowerShell 7 on Windows x64 and the cached, pinned toolchain (see docs/UPDATE_RECOVERY.md).
param(
    [ValidateSet('Preflight','Prepare','Build','Package','Accept','All')][string]$Stage = 'Preflight',
    [string]$Case, [string]$VersionA = '0.6.1', [string]$VersionB = '0.6.2',
    [string]$RuntimeDirectory, [string]$CompilerDirectory, [string]$DotnetPath, [string]$PackagerDirectory
)
. "$PSScriptRoot/update-qualification/common.ps1"
if (-not $IsWindows -or [IntPtr]::Size -ne 8 -or $PSVersionTable.PSVersion.Major -lt 7) {
    throw 'Windows x64 and PowerShell 7 are required'
}
if ($env:CARGO_BUILD_TARGET) { throw 'Unset CARGO_BUILD_TARGET; this runner qualifies the native Windows x64 build' }
$resuming = $Stage -in @('Build','Package','Accept')
if ($resuming) {
    if (-not $Case) { throw 'An existing -Case is required for this stage' }
    if ($RuntimeDirectory -or $CompilerDirectory -or $DotnetPath -or $PackagerDirectory -or
        $PSBoundParameters.ContainsKey('VersionA') -or $PSBoundParameters.ContainsKey('VersionB')) {
        throw 'Existing cases use their frozen plan; do not override inputs or versions'
    }
    $Case = Assert-ChildPath $Case $qualificationBase
    $plan = Read-QualificationPlan $Case
    $inputs = $plan.inputs
} else {
    if (-not $Case) { $Case = Join-Path $qualificationBase ('gui-' + [guid]::NewGuid().ToString('N')) }
    $Case = Assert-ChildPath $Case $qualificationBase
    if (Test-Path -LiteralPath $Case) { throw 'Case already exists; use a fresh case or an explicit continuation stage' }
    Assert-VersionPair $VersionA $VersionB
    $inputs = [ordered]@{
        runtime = if ($RuntimeDirectory) { $RuntimeDirectory } else { "$workspace/dist/noh" }
        compiler = if ($CompilerDirectory) { $CompilerDirectory } else { "$qualificationBase/llvm-mingw-20260922-ucrt-x86_64/bin" }
        dotnet = if ($DotnetPath) { $DotnetPath } else { "$qualificationBase/dotnet8/dotnet.exe" }
        packager = if ($PackagerDirectory) { $PackagerDirectory } else { "$qualificationBase/vpk" }
    }
    foreach ($key in @($inputs.Keys)) { $inputs[$key] = Assert-PlainPath $inputs[$key] }
    $plan = [ordered]@{schema_version=1; case=$Case; version_a=$VersionA; version_b=$VersionB;
        package_id='NohGuiUpdateQualification'; requires_external_repair=$true; inputs=$inputs;
        scope='Local authenticated GUI update and external manual repair; no remote publication'}
}

function Test-Prerequisites {
    Assert-NoRecordedOwner "$qualificationBase/active-accept.json"
    $active = @(Get-Process -Name cargo,rustc,noh-update-guard,Update,update_x64,update-measure -ErrorAction SilentlyContinue)
    if ($active.Count) { throw "A build or updater owner is active: $($active.Id -join ', ')" }
    foreach ($process in Get-Process) {
        if ($process.Path -and $process.Path.StartsWith($qualificationBase + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw "A qualification executable remains active: $($process.Id) $($process.Path)"
        }
    }
    foreach ($command in @('git','cargo','rustc','pwsh')) { [void](Get-Command $command -CommandType Application -ErrorAction Stop) }
    $required = @("$($inputs.runtime)/bin/ffmpeg.exe", "$($inputs.runtime)/bin/libmpv-2.dll",
        "$($inputs.compiler)/clang.exe", "$($inputs.compiler)/llvm-ar.exe", $inputs.dotnet,
        "$($inputs.packager)/tools/net8.0/any/vpk.dll", "$($inputs.packager)/vendor/update_x64.exe",
        "$($inputs.packager)/vpk.nuspec")
    foreach ($path in $required) {
        [void](Assert-PlainPath $path)
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing prerequisite: $path" }
    }
    $rust = Invoke-RecordedProcess -File (Get-Command rustc).Source -Arguments @('-vV') -Directory $workspace -TimeoutSeconds 15 -PassOutput
    if ($rust -notmatch '(?m)^host: x86_64-pc-windows-gnu\r?$' -or $rust -notmatch '(?m)^release: ([0-9.]+)') { throw 'The qualified toolchain is native Windows GNU Rust' }
    if ([version]$Matches[1] -lt [version]'1.95.0') { throw 'Rust 1.95 or newer is required' }
    $runtimes = Invoke-RecordedProcess -File $inputs.dotnet -Arguments @('--list-runtimes') -Directory $workspace -TimeoutSeconds 15 -PassOutput
    if ($runtimes -notmatch 'Microsoft.NETCore.App 8\.') { throw '.NET 8 runtime is required by the pinned packager' }
    if ((Get-Content "$workspace/.cargo/config.toml" -Raw) -notmatch 'dev\s*=\s*"run --profile qa --locked --example dev --"') { throw 'The optimized QA cargo dev alias is required' }
    [xml]$nuspec = Get-Content "$($inputs.packager)/vpk.nuspec" -Raw
    if ($nuspec.package.metadata.version -ne '1.2.161') { throw 'This runner requires Velopack 1.2.161' }
    if ((Get-FileHash "$($inputs.packager)/vendor/update_x64.exe").Hash -ne $helperSha256) { throw 'Pinned Velopack helper hash mismatch' }
    $preview = Get-Content "$workspace/assets/preview-runtime.json" -Raw | ConvertFrom-Json
    if ((Get-FileHash "$($inputs.runtime)/bin/libmpv-2.dll").Hash -ne $preview.sha256) { throw 'Preview runtime hash mismatch' }
    $speech = Get-Content "$workspace/assets/speech-bundle.json" -Raw | ConvertFrom-Json
    foreach ($item in $speech.sha256.PSObject.Properties) {
        if ([IO.Path]::GetFileName($item.Name) -ne $item.Name) { throw 'Invalid speech manifest filename' }
        $relative = if ($item.Name.EndsWith('-LICENSE.txt')) { "licenses/$($item.Name)" } else { "bin/speech/$($item.Name)" }
        $path = Assert-PlainPath (Join-Path $inputs.runtime $relative)
        if ((Get-FileHash -LiteralPath $path).Hash -ne $item.Value) { throw "Speech runtime hash mismatch: $relative" }
    }
    $runtimeBytes = (Get-ChildItem -LiteralPath $inputs.runtime -File -Recurse | Measure-Object Length -Sum).Sum
    # Two bundles, full packages, retained archives, native installation, preserved B and build headroom.
    $requiredBytes = [long](12 * $runtimeBytes + 8GB)
    $freeBytes = ([IO.DriveInfo]::new([IO.Path]::GetPathRoot($Case))).AvailableFreeSpace
    if ($freeBytes -lt $requiredBytes) { throw "Insufficient free space: need at least $requiredBytes bytes, have $freeBytes" }
    [ordered]@{passed=$true; case=$Case; stage=$Stage; inputs=$inputs; free_bytes=$freeBytes;
        required_bytes=$requiredBytes; package_version='1.2.161'; helper_sha256=$helperSha256;
        native_execution_performed=$false; offline_dependencies='Required; Cargo fails offline if a cached dependency is missing'}
}

if ($Stage -eq 'Preflight') { Test-Prerequisites | ConvertTo-Json -Depth 6; return }
# The lock serializes this runner's shared target. An unrelated Cargo process is refused by preflight.
[void](Assert-PlainPath $qualificationBase)
[void][IO.Directory]::CreateDirectory($qualificationBase)
[void](Assert-PlainPath "$qualificationBase/qualification.lock")
$lock = [IO.File]::Open("$qualificationBase/qualification.lock", 'OpenOrCreate', 'ReadWrite', 'None')
$timer = [Diagnostics.Stopwatch]::StartNew()
$names = if ($Stage -eq 'All') { @('Prepare','Build','Package','Accept') } else { @($Stage) }
$steps = @([ordered]@{name='Preflight';status='skipped';seconds=0;exit=$null})
foreach ($name in $names) { $steps += [ordered]@{name=$name;status='skipped';seconds=0;exit=$null} }
$result = [ordered]@{schema_version=1;case=$Case;status='failed';started_utc=[DateTime]::UtcNow.ToString('o');
    seconds=0;steps=$steps;error=$null;native_execution_performed=$false}
$attempt = Join-Path $qualificationBase ('attempt-' + [guid]::NewGuid().ToString('N'))
$currentStep = $steps[0]
try {
    $stepTimer = [Diagnostics.Stopwatch]::StartNew()
    $preflight = Test-Prerequisites
    $currentStep.status='passed'; $currentStep.exit=0; $currentStep.seconds=$stepTimer.Elapsed.TotalSeconds
    if (-not $resuming) {
        [void][IO.Directory]::CreateDirectory($Case)
        Write-Json $plan "$Case/qualification-plan.json"
    }
    Write-Json $preflight "$Case/preflight.json"
    $attempt = Join-Path $Case ([IO.Path]::GetFileName($attempt))
    $pwsh = (Get-Command pwsh -CommandType Application).Source
    foreach ($currentStep in $steps | Select-Object -Skip 1) {
        $stepTimer = [Diagnostics.Stopwatch]::StartNew()
        $name = $currentStep.name
        $script = Join-Path $PSScriptRoot "update-qualification/$($name.ToLowerInvariant()).ps1"
        $limit = switch ($name) { 'Prepare' {1800} 'Build' {7200} 'Package' {1800} 'Accept' {1200} }
        if ($name -ne 'Prepare') { Assert-Prepared $Case }
        if ($name -eq 'Accept') { $result.native_execution_performed=$true }
        $ownerMarker = if ($name -eq 'Accept') { "$qualificationBase/active-accept.json" } else { $null }
        Invoke-RecordedProcess -File $pwsh -Arguments @('-NoProfile','-File',$script,'-Case',$Case) `
            -Directory $workspace -Log "$attempt-$name" -TimeoutSeconds $limit -PreserveOnTimeout:($name -eq 'Accept') -OwnerMarker $ownerMarker
        $currentStep.status='passed'; $currentStep.exit=0; $currentStep.seconds=$stepTimer.Elapsed.TotalSeconds
    }
    $result.status='passed'
} catch {
    $result.error=$_.Exception.Message
    $currentStep.status='failed'; $currentStep.seconds=$stepTimer.Elapsed.TotalSeconds
    $processRecord = "$attempt-$($currentStep.name).process.json"
    if (Test-Path $processRecord) { $currentStep.exit=(Get-Content $processRecord -Raw | ConvertFrom-Json).exit }
    throw
} finally {
    $result.seconds=$timer.Elapsed.TotalSeconds
    Write-Json $result "$attempt-result.json"
    Write-Output "Qualification $($result.status): $attempt-result.json"
    $lock.Dispose()
}
