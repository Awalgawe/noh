# Local negative acceptance test for the pinned Velopack apply boundary.
# Does not install NOH, publish releases, or access user application data.
[CmdletBinding()]
param(
    [string]$VendorDirectory = '.mcp-dev/update-feasibility/vpk/vendor',
    [switch]$Guarded
)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'This probe requires PowerShell 7 on Windows.' }
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$helper = (Resolve-Path -LiteralPath (Join-Path $projectRoot "$VendorDirectory/update_x64.exe")).Path
$expectedHelperHash = 'ACEFB4A2CB46CC77ED3C2364DB17F8BD2A25E2197CFEAE56CD85E88A7AC21AC5'
if ((Get-FileHash -LiteralPath $helper -Algorithm SHA256).Hash -ne $expectedHelperHash) {
    throw 'Unexpected updater binary; this probe is pinned to Velopack 1.2.161.'
}
$probeBase = [IO.Path]::GetFullPath((Join-Path $projectRoot '.mcp-dev/update-feasibility'))
$runDir = Join-Path $probeBase ('run-' + [Guid]::NewGuid().ToString('N'))
if (-not $runDir.StartsWith($probeBase + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Probe path escaped its workspace.'
}
$installDir = Join-Path $runDir 'installation'
$currentDir = Join-Path $installDir 'current'
$packagesDir = Join-Path $installDir 'packages'
$payloadDir = Join-Path $runDir 'payload'
$libDir = Join-Path $payloadDir 'lib/app'
foreach ($path in @($currentDir, $packagesDir, $libDir, (Join-Path $runDir 'profile'))) {
    New-Item -ItemType Directory -Path $path -Force | Out-Null
}
Set-Content -LiteralPath (Join-Path $installDir '.portable') -Value ''
Copy-Item -LiteralPath $helper -Destination (Join-Path $installDir 'Update.exe')

function New-Manifest([string]$Version) {
    return "<package><metadata><id>NohUpdateBoundaryProbe</id><version>$Version</version><title>NOH isolated update probe</title><mainExe>probe.exe</mainExe><os>win</os><machineArchitecture>x64</machineArchitecture><channel>win-x64-probe</channel></metadata></package>"
}
Set-Content -LiteralPath (Join-Path $currentDir 'sq.version') -Value (New-Manifest '1.0.0') -Encoding utf8
Set-Content -LiteralPath (Join-Path $currentDir 'marker.txt') -Value 'old-version' -Encoding utf8
Set-Content -LiteralPath (Join-Path $payloadDir 'probe.nuspec') -Value (New-Manifest '1.0.1') -Encoding utf8
# A harmless executable accepts updater hooks and never reads or writes user data.
$rustSource = Join-Path $runDir 'probe.rs'
Set-Content -LiteralPath $rustSource -Value 'fn main() { if std::env::args().nth(1).as_deref() == Some("--hold") { std::thread::sleep(std::time::Duration::from_secs(2)); } println!("NOH isolated update probe"); }' -Encoding utf8
& rustc $rustSource --edition 2021 -C linker=rust-lld -C linker-flavor=ld.lld -o (Join-Path $currentDir 'probe.exe') *> (Join-Path $runDir 'compile.log')
if ($LASTEXITCODE -ne 0) { throw "Probe compilation failed: $runDir/compile.log" }
Copy-Item -LiteralPath (Join-Path $currentDir 'probe.exe') -Destination (Join-Path $libDir 'probe.exe')
Set-Content -LiteralPath (Join-Path $libDir 'marker.txt') -Value 'intended-new-version' -Encoding utf8
$intendedPackage = Join-Path $runDir 'intended.nupkg'
[IO.Compression.ZipFile]::CreateFromDirectory($payloadDir, $intendedPackage)
$intendedHash = (Get-FileHash -LiteralPath $intendedPackage -Algorithm SHA256).Hash
# Record a baseline then supply different bytes at the install boundary.
# No caller-side signature verification is performed in the non-guarded mode.
Set-Content -LiteralPath (Join-Path $libDir 'marker.txt') -Value 'substituted-unsigned-payload' -Encoding utf8
$substitutedPackage = Join-Path $packagesDir 'NohUpdateBoundaryProbe-1.0.1-full.nupkg'
[IO.Compression.ZipFile]::CreateFromDirectory($payloadDir, $substitutedPackage)
$substitutedHash = (Get-FileHash -LiteralPath $substitutedPackage -Algorithm SHA256).Hash
if ($intendedHash -eq $substitutedHash) { throw 'Invalid fixture: substitution did not change the package.' }

$guardHandles = [Collections.Generic.List[IDisposable]]::new()
$signatureVerified = $false
$invalidSignatureRejected = $false
$writeBlocked = $false
$renameBlocked = $false
$parentRenameBlocked = $false
$packageToApply = $substitutedPackage
if ($Guarded) {
    # Fixture cryptography only. A production release key must not be generated here.
    $key = [Security.Cryptography.RSA]::Create(2048)
    $algorithm = [Security.Cryptography.HashAlgorithmName]::SHA256
    $padding = [Security.Cryptography.RSASignaturePadding]::Pss
    $signature = $key.SignHash([Convert]::FromHexString($intendedHash), $algorithm, $padding)
    $invalidSignatureRejected = -not $key.VerifyHash([Convert]::FromHexString($substitutedHash), $signature, $algorithm, $padding)
    $packageToApply = Join-Path $packagesDir 'NohUpdateBoundaryProbe-1.0.1-signed-full.nupkg'
    Copy-Item -LiteralPath $intendedPackage -Destination $packageToApply
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class ProbeDirectoryGuard {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(string path, uint access, uint share,
        IntPtr security, uint creation, uint flags, IntPtr template);
    public static SafeFileHandle Lock(string path) {
        var handle = CreateFileW(path, 0, 3, IntPtr.Zero, 3, 0x02000000, IntPtr.Zero);
        if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
        return handle;
    }
}
'@
    # Keep every ancestor through the workspace root from being renamed, and
    # keep the package itself readable but unwritable/undeletable during apply.
    $ancestor = $packagesDir
    while ($true) {
        $guardHandles.Add([ProbeDirectoryGuard]::Lock($ancestor))
        if ($ancestor -eq $projectRoot) { break }
        $ancestor = [IO.Path]::GetDirectoryName($ancestor)
        if (-not $ancestor) { throw 'Guard ancestor chain missed workspace root.' }
    }
    $packageGuard = [IO.FileStream]::new($packageToApply, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $guardHandles.Add($packageGuard)
    $digest = [Security.Cryptography.SHA256]::HashData($packageGuard)
    $signatureVerified = $key.VerifyHash($digest, $signature, $algorithm, $padding)
    if (-not $signatureVerified -or -not $invalidSignatureRejected) { throw 'Signature fixture failed.' }
    try { [IO.File]::WriteAllText($packageToApply, 'unexpected overwrite') } catch { $writeBlocked = $true }
    try { [IO.File]::Move($packageToApply, $packageToApply + '.moved') } catch { $renameBlocked = $true }
    try { [IO.Directory]::Move($packagesDir, $packagesDir + '.moved') } catch { $parentRenameBlocked = $true }
    if (-not ($writeBlocked -and $renameBlocked -and $parentRenameBlocked)) {
        throw 'The guard did not block all mutation attempts; do not apply.'
    }
}

$startInfo = [Diagnostics.ProcessStartInfo]::new()
$startInfo.FileName = $helper
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$startInfo.WorkingDirectory = $runDir
foreach ($argument in @('--silent', '--verbose', '--rootDir', $installDir, '--packageDir', $packagesDir,
    '--log', (Join-Path $runDir 'apply.log'), 'apply', '--norestart', '--package', $packageToApply)) {
    $startInfo.ArgumentList.Add($argument)
}
$clientProcess = $null
if ($Guarded) {
    $clientInfo = [Diagnostics.ProcessStartInfo]::new()
    $clientInfo.FileName = Join-Path $currentDir 'probe.exe'
    $clientInfo.UseShellExecute = $false
    $clientInfo.CreateNoWindow = $true
    $clientInfo.ArgumentList.Add('--hold')
    $clientProcess = [Diagnostics.Process]::Start($clientInfo)
    # The helper's waitPid can continue on access failure. The supervisor must
    # establish exit itself, and must not delegate a fail-closed wait to it.
    if (-not $clientProcess.WaitForExit(10000) -or $clientProcess.ExitCode -ne 0) {
        throw 'Client exit was not established; refusing to invoke the helper.'
    }
}
foreach ($name in @('APPDATA', 'LOCALAPPDATA', 'TEMP', 'TMP')) {
    $startInfo.Environment[$name] = Join-Path $runDir 'profile'
}
$process = [Diagnostics.Process]::new()
$process.StartInfo = $startInfo
$process.Start() | Out-Null
$stdoutTask = $process.StandardOutput.ReadToEndAsync()
$stderrTask = $process.StandardError.ReadToEndAsync()
$timedOut = -not $process.WaitForExit(45000)
if ($timedOut) {
    # This handle identifies only the helper launched by this probe.
    $process.Kill($true)
    $process.WaitForExit()
}
$exitCode = $process.ExitCode
Set-Content -LiteralPath (Join-Path $runDir 'stdout.log') -Value $stdoutTask.GetAwaiter().GetResult()
Set-Content -LiteralPath (Join-Path $runDir 'stderr.log') -Value $stderrTask.GetAwaiter().GetResult()
$marker = $null
$markerReadError = $null
try {
    $marker = (Get-Content -LiteralPath (Join-Path $currentDir 'marker.txt') -Raw).Trim()
} catch {
    # Interrupted replacement can leave no current directory; still save evidence.
    $markerReadError = $_.Exception.Message
}
$result = [ordered]@{
    engine_version = '1.2.161'
    helper_sha256 = $expectedHelperHash
    intended_package_sha256 = $intendedHash
    substituted_package_sha256 = $substitutedHash
    updater_exit_code = $exitCode
    installed_marker = $marker
    marker_read_error = $markerReadError
    unsigned_package_applied_by_direct_helper = (-not $timedOut -and $exitCode -eq 0 -and $marker -eq 'substituted-unsigned-payload')
    caller_signature_verified = $signatureVerified
    invalid_signature_rejected = $invalidSignatureRejected
    guarded = [bool]$Guarded
    overwrite_blocked = $writeBlocked
    package_rename_blocked = $renameBlocked
    parent_directory_rename_blocked = $parentRenameBlocked
    authenticated_guarded_package_applied = ($Guarded -and -not $timedOut -and $exitCode -eq 0 -and $signatureVerified -and $marker -eq 'intended-new-version')
    client_exited_while_guard_alive = ($Guarded -and $clientProcess.HasExited -and $clientProcess.ExitCode -eq 0)
    supervisor_waited_for_client = [bool]$Guarded
    updater_timed_out = $timedOut
    qualification = 'Windows helper boundary fixture; not a NOH update, production key or elevated installation'
    evidence_directory = $runDir
}
$result | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $runDir 'result.json') -Encoding utf8
$result | ConvertTo-Json
foreach ($handle in $guardHandles) { $handle.Dispose() }
if ($Guarded) { $key.Dispose() }
if ($timedOut) { throw "Probe updater timed out; evidence retained in $runDir" }
if ($Guarded -and -not ($result.authenticated_guarded_package_applied -and $result.client_exited_while_guard_alive)) {
    throw 'Guarded install failed; inspect evidence before qualifying the handoff.'
}
if (-not $Guarded -and -not $result.unsigned_package_applied_by_direct_helper) {
    throw 'Expected boundary gap not reproduced; inspect evidence before deciding engine feasibility.'
}
