# Shared only by the local Windows qualification runner and its focused tests.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$qualificationBase = Join-Path $workspace '.mcp-dev/update-feasibility'
$helperSha256 = 'ACEFB4A2CB46CC77ED3C2364DB17F8BD2A25E2197CFEAE56CD85E88A7AC21AC5'

function Assert-PlainPath([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    $cursor = $full
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            if ((Get-Item -Force -LiteralPath $cursor).Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Reparse path refused: $cursor"
            }
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
    return $full
}

function Assert-ChildPath([string]$Path, [string]$Parent) {
    $full = Assert-PlainPath $Path
    $root = (Assert-PlainPath $Parent).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $full.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Path must be inside $Parent : $full"
    }
    return $full
}

function Write-Json($Value, [string]$Path) {
    $Value | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $Path -Encoding utf8NoBOM
}

function Assert-VersionPair([string]$A, [string]$B) {
    foreach ($version in @($A, $B)) {
        if ($version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
            throw 'Qualification versions must be three numeric components without leading zeroes'
        }
    }
    if ([version]$A -ge [version]$B) { throw 'Version B must be newer than A' }
}

function Invoke-RecordedProcess {
    param([string]$File, [string[]]$Arguments, [string]$Directory,
          [string]$Log, [int]$TimeoutSeconds = 300, [switch]$PreserveOnTimeout,
          [string]$OwnerMarker, [switch]$PassOutput)
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $record = [ordered]@{ executable=$File; exit=$null; seconds=0; timed_out=$false; pid=$null; error=$null }
    $process = $null
    $outFile = $null; $errFile = $null
    try {
        $psi = [Diagnostics.ProcessStartInfo]::new($File)
        $psi.WorkingDirectory = $Directory
        $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
        $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
        foreach ($argument in $Arguments) { $psi.ArgumentList.Add($argument) }
        $process = [Diagnostics.Process]::Start($psi)
        $record.pid = $process.Id
        if ($OwnerMarker) {
            Write-Json @{pid=$process.Id;start_ticks=$process.StartTime.ToUniversalTime().Ticks;executable=$File;directory=$Directory} $OwnerMarker
        }
        if ($Log) {
            [void](Assert-PlainPath "$Log.stdout.log"); [void](Assert-PlainPath "$Log.stderr.log")
            $outFile = [IO.File]::Open("$Log.stdout.log", 'Create', 'Write', 'ReadWrite')
            $errFile = [IO.File]::Open("$Log.stderr.log", 'Create', 'Write', 'ReadWrite')
            $stdout = $process.StandardOutput.BaseStream.CopyToAsync($outFile)
            $stderr = $process.StandardError.BaseStream.CopyToAsync($errFile)
        } else {
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
        }
        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            $record.timed_out = $true
            if (-not $PreserveOnTimeout) { $process.Kill($true); [void]$process.WaitForExit(5000) }
            throw "Process deadline ($TimeoutSeconds seconds), PID $($record.pid); preserve=$PreserveOnTimeout"
        }
        $record.exit = $process.ExitCode
        if (-not [Threading.Tasks.Task]::WaitAll([Threading.Tasks.Task[]]@($stdout,$stderr), 5000)) {
            throw 'Output stream deadline; a descendant may still own the redirected streams'
        }
        if ($record.exit -ne 0) { throw "Process failed with exit $($record.exit); see $Log.stderr.log" }
        if ($PassOutput) {
            if ($Log) {
                # Close the completed writer before ReadAllText opens a reader
                # whose sharing mode would otherwise deny that existing writer.
                $outFile.Dispose(); $outFile = $null
                [IO.File]::ReadAllText("$Log.stdout.log")
            } else { $stdout.Result }
        }
    } catch {
        $record.error = $_.Exception.Message
        throw
    } finally {
        $record.seconds = $timer.Elapsed.TotalSeconds
        if ($Log) { Write-Json $record "$Log.process.json" }
        # Partial logs survive failures and deadlines. Preserved children remain refusal owners.
        if ($outFile) { $outFile.Dispose() }
        if ($errFile) { $errFile.Dispose() }
        if ($process) { $process.Dispose() }
    }
}

function Assert-NoRecordedOwner([string]$Marker) {
    if (-not (Test-Path -LiteralPath $Marker)) { return }
    [void](Assert-PlainPath $Marker)
    $owner = Get-Content -LiteralPath $Marker -Raw | ConvertFrom-Json
    $process = Get-Process -Id $owner.pid -ErrorAction SilentlyContinue
    if ($process -and $process.StartTime.ToUniversalTime().Ticks -eq $owner.start_ticks) {
        throw "Previous acceptance process remains active: $($owner.pid); inspect $Marker before resuming"
    }
}

function Read-QualificationPlan([string]$Case) {
    $Case = Assert-ChildPath $Case $qualificationBase
    [void](Assert-PlainPath "$Case/qualification-plan.json")
    $plan = Get-Content -LiteralPath "$Case/qualification-plan.json" -Raw | ConvertFrom-Json
    if ($plan.schema_version -ne 1 -or $plan.case -ne $Case -or $plan.package_id -ne 'NohGuiUpdateQualification' -or -not $plan.requires_external_repair) {
        throw 'Unsupported qualification plan; create a new case with the repository runner'
    }
    Assert-VersionPair $plan.version_a $plan.version_b
    return $plan
}

function Assert-Prepared([string]$Case) {
    $prepared = Get-Content "$Case/prepared.json" -Raw | ConvertFrom-Json
    foreach ($item in @(@('tools/update-release.exe','verifier_sha256'), @('public-keys.json','public_keys_sha256'))) {
        $path = Assert-ChildPath "$Case/$($item[0])" $Case
        if ((Get-FileHash -LiteralPath $path).Hash -ne $prepared.($item[1])) { throw "Prepared input changed: $($item[0])" }
    }
    Assert-Snapshot $Case 'A'
    Assert-Snapshot $Case 'B'
    Assert-Harness $Case $workspace
}

function Assert-Harness([string]$Case, [string]$CurrentWorkspace) {
    $inventory = Get-Content "$Case/source-inventory.json" -Raw | ConvertFrom-Json
    $harness = @($inventory | Where-Object { $_.path -match '^tools/(update-qualification\.ps1$|update-qualification/|update-repair-qualification\.ps1$)' })
    if ($harness.Count -lt 7) { throw 'Incomplete frozen qualification harness' }
    foreach ($item in $harness) {
        $path = Assert-ChildPath "$CurrentWorkspace/$($item.path)" $CurrentWorkspace
        if ((Get-FileHash -LiteralPath $path).Hash -ne $item.sha256_source) { throw "Qualification harness changed: $($item.path); use a fresh case" }
    }
}

function Set-QualificationEnvironment($Plan) {
    # Stage scripts run in child PowerShell processes; the caller environment is unchanged.
    foreach ($key in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'NOH_*' })) {
        Remove-Item -LiteralPath "Env:$($key.Name)"
    }
    $env:CC = Join-Path $Plan.inputs.compiler 'clang.exe'
    $env:AR = Join-Path $Plan.inputs.compiler 'llvm-ar.exe'
    $env:CARGO_TARGET_DIR = Join-Path $workspace 'target'
    $env:NOH_LIBMPV = Join-Path $Plan.inputs.runtime 'bin/libmpv-2.dll'
    $env:NOH_UPDATE_QUALIFICATION_ROOT = $Plan.case
    $env:NOH_UPDATE_PACKAGE_ID = $Plan.package_id
    $env:NOH_UPDATE_CHANNEL = 'stable'
    $env:NOH_UPDATE_FEED_URL = 'https://github.com/example/noh/releases/download/gui-local/envelope.json'
    if (Test-Path "$($Plan.case)/public-keys.json") {
        $env:NOH_UPDATE_TRUST_JSON = Get-Content "$($Plan.case)/public-keys.json" -Raw
    }
}

function Set-SnapshotVersion([string]$Directory, [string]$Version) {
    foreach ($file in @('Cargo.toml', 'Cargo.lock')) {
        $path = Join-Path $Directory $file
        $content = [IO.File]::ReadAllText($path)
        $pattern = '(?m)(^name = "noh"\r?\nversion = ")[^"]+("\r?$)'
        if ([regex]::Matches($content, $pattern).Count -ne 1) { throw "Ambiguous NOH version in $file" }
        $content = [regex]::Replace($content, $pattern, { param($match) $match.Groups[1].Value + $Version + $match.Groups[2].Value })
        [IO.File]::WriteAllText($path, $content)
    }
}

function Assert-Snapshot([string]$Case, [string]$Variant) {
    $inventory = Get-Content "$Case/source-inventory.json" -Raw | ConvertFrom-Json
    $root = Assert-PlainPath "$Case/source-$Variant"
    $actual = @(Get-ChildItem -LiteralPath $root -Recurse -Force | ForEach-Object {
        $relative = [IO.Path]::GetRelativePath($root, $_.FullName).Replace('\','/')
        if ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Reparse source refused: $relative" }
        if ($relative -eq 'dist' -or $relative.StartsWith('dist/')) { return }
        if (-not $_.PSIsContainer) { $relative }
    } | Sort-Object)
    if (Compare-Object @($inventory.path | Sort-Object) $actual) { throw "Frozen source file set changed: $Variant" }
    foreach ($item in $inventory) {
        $path = Assert-ChildPath "$Case/source-$Variant/$($item.path)" "$Case/source-$Variant"
        if ((Get-FileHash -LiteralPath $path).Hash -ne $item."sha256_$Variant") {
            throw "Frozen source changed: $Variant/$($item.path)"
        }
    }
}
