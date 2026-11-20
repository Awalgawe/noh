# Shared by the Windows QA, installer-tool and CI measurement scripts.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))

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
