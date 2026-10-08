# Manual recovery for disposable NOH A/B qualification fixtures only.
param([Parameter(Mandatory)][string]$Case,[Parameter(Mandatory)][string]$Trial)
$ErrorActionPreference='Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class RecoveryAncestorProtection {
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)]
 static extern SafeFileHandle CreateFile(string path,uint access,uint share,IntPtr security,uint disposition,uint flags,IntPtr template);
 public static SafeFileHandle Open(string path) {
  var handle=CreateFile(path,0x80,3,IntPtr.Zero,3,0x02000000,IntPtr.Zero);
  if(handle.IsInvalid)throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
  return handle;
 }
}
'@
$workspace=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$case=(Resolve-Path -LiteralPath $Case).Path
$allowed=(Resolve-Path (Join-Path $workspace '.mcp-dev/update-feasibility')).Path
if(-not $case.StartsWith($allowed+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Case must be a disposable child of the project qualification directory'}
$trialPath=(Resolve-Path -LiteralPath $Trial).Path
if(-not $trialPath.StartsWith($case+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Recovery is restricted to this disposable qualification case'}
$root=(Resolve-Path -LiteralPath "$trialPath/installation").Path
if(-not(Test-Path "$root/.portable")){throw 'Portable marker required'}
# The caller must first observe termination of the previous owned guardian/job.
# A waiting, committed guardian must not resume after the recovery lease is released.
foreach($process in Get-Process -Name 'noh-update-guard*','update-measure','update_x64' -ErrorAction SilentlyContinue){
 $path=$process.Path
 if($path -and $path.StartsWith($workspace+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Previous project guardian/helper job is still running; recovery refused'}
}
foreach($process in Get-Process){
 $path=$process.Path
 if($path -and $path.StartsWith($root+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw "Installed process $($process.Id) is still running; recovery refused"}
}
$package="$case/releases/NohUpdateQualification-0.1.1-win-x64-stable-full.nupkg"
$recovery=Join-Path $root ('recovery-'+[guid]::NewGuid().ToString('N'))
$protections=[Collections.Generic.List[IDisposable]]::new()
function Protect-Ancestors([string]$Path){
 $directory=[IO.DirectoryInfo]::new([IO.Path]::GetFullPath($Path))
 while($directory){
  if(($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Recovery reparse ancestor rejected'}
  $protections.Add([RecoveryAncestorProtection]::Open($directory.FullName))
  $directory=$directory.Parent
 }
}
$keys="$case/public-keys.json";$envelope="$case/envelope-0.1.1.json"
try {
 Protect-Ancestors $root
 Protect-Ancestors ([IO.Path]::GetDirectoryName($package))
 foreach($path in @($keys,$envelope,"$root/.portable")){
  if(((Get-Item -LiteralPath $path).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Recovery reparse input rejected'}
  $protections.Add([IO.File]::Open($path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read))
 }
 if(((Get-Item -LiteralPath $package).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Recovery reparse package rejected'}
$lease=[IO.File]::Open("$root/.noh-update-runtime.lock",[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
try {
$stream=[IO.File]::Open($package,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
try {
 $verification=& "$workspace/target/qa/examples/update-release.exe" verify --public-keys $keys --envelope $envelope --package $package --package-id NohUpdateQualification --expected-version 0.1.1 2>&1
 if($LASTEXITCODE -ne 0){throw "Retained package authentication failed: $verification"}
 New-Item -ItemType Directory -Path $recovery|Out-Null
 Add-Type -AssemblyName System.IO.Compression
 $zip=[IO.Compression.ZipArchive]::new($stream,[IO.Compression.ZipArchiveMode]::Read,$true)
 try {
  $names=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase);$bytes=0L;$files=0
  foreach($entry in $zip.Entries){
   if($entry.FullName -notmatch '^lib/[^/]+/(.+)$'){continue}
   $relative=$Matches[1];if($relative.EndsWith('/')){continue}
   if($relative.Contains('\') -or $relative.Contains(':') -or $relative.Split('/') -contains '..' -or -not $names.Add($relative)){throw 'Unsafe or duplicate recovery archive path'}
   if((($entry.ExternalAttributes -shr 16) -band 0xF000) -eq 0xA000){throw 'Recovery symlink rejected'}
   $destination=[IO.Path]::GetFullPath((Join-Path $recovery $relative))
   if(-not $destination.StartsWith($recovery+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Recovery path escapes staging directory'}
   $bytes+=$entry.Length;$files++;if($bytes -gt 4GB -or $files -gt 4096){throw 'Recovery extraction bound exceeded'}
   [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))|Out-Null
   [IO.Compression.ZipFileExtensions]::ExtractToFile($entry,$destination,$false)
  }
 }finally{$zip.Dispose()}
}finally{$stream.Dispose()}
$manifest=Get-Content "$recovery/manifest.json" -Raw|ConvertFrom-Json
if($manifest.build.package_version -ne '0.1.1'){throw 'Recovered identity mismatch'}
foreach($property in $manifest.sha256.PSObject.Properties){if((Get-FileHash -LiteralPath (Join-Path $recovery $property.Name)).Hash.ToLowerInvariant() -ne $property.Value){throw "Recovered file mismatch: $($property.Name)"}}
# Preserve any interrupted current tree for inspection. No files are deleted.
$quarantine=$null
if(Test-Path "$root/current"){
 if(((Get-Item -LiteralPath "$root/current").Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Recovery current reparse rejected'}
 $quarantine=Join-Path $root ('interrupted-'+[guid]::NewGuid().ToString('N'));Move-Item -LiteralPath "$root/current" -Destination $quarantine
}
Move-Item -LiteralPath $recovery -Destination "$root/current"
}finally{$lease.Dispose()}
}finally{foreach($protection in $protections){$protection.Dispose()}}
$info=& "$root/current/bin/noh-cli.exe" --build-info|ConvertFrom-Json
if($LASTEXITCODE -ne 0 -or $info.package_version -ne '0.1.1'){throw 'Recovered CLI did not start as A'}
if(Test-Path "$trialPath/before-project-hashes.json"){
 $preserved=Get-Content "$trialPath/before-project-hashes.json" -Raw|ConvertFrom-Json
 foreach($property in $preserved.PSObject.Properties){if((Get-FileHash -LiteralPath $property.Name).Hash -ne $property.Value){throw "User data changed: $($property.Name)"}}
}
if(Test-Path "$trialPath/project-data/project.json"){
 $capture="$trialPath/ui-recovered";New-Item -ItemType Directory -Path $capture|Out-Null
 $psi=[Diagnostics.ProcessStartInfo]::new("$root/current/noh.exe");$psi.UseShellExecute=$false;$psi.CreateNoWindow=$true;$psi.RedirectStandardOutput=$true;$psi.RedirectStandardError=$true
 foreach($name in @('NOH_LANGUAGE','NOH_CAPTURE_STATE','NOH_CAPTURE_TWEAKS','NOH_CAPTURE_PROJECT_ACTION')){[void]$psi.Environment.Remove($name)}
 $psi.Environment['APPDATA']="$case/runtime-profile";$psi.Environment['LOCALAPPDATA']="$case/runtime-profile"
 $psi.Environment['NOH_CAPTURE_UI']="$capture/capture.ppm";$psi.Environment['NOH_CAPTURE_PROJECT']="$trialPath/project-data/project.json";$psi.Environment['NOH_FFMPEG']="$root/current/bin/ffmpeg.exe";$psi.Environment['NOH_CAPTURE_WIDTH']='980';$psi.Environment['NOH_CAPTURE_HEIGHT']='850';$psi.Environment['NOH_CAPTURE_THEME']='dark';$psi.Environment['NOH_CAPTURE_SCALE']='1'
 $gui=[Diagnostics.Process]::Start($psi);$out=$gui.StandardOutput.ReadToEndAsync();$err=$gui.StandardError.ReadToEndAsync()
 if(-not $gui.WaitForExit(30000)){$gui.Kill();throw 'Recovered GUI capture deadline'}
 $out.GetAwaiter().GetResult()|Set-Content "$capture/stdout.log";$err.GetAwaiter().GetResult()|Set-Content "$capture/stderr.log"
 if($gui.ExitCode -ne 0){throw 'Recovered GUI failed to start'}
 $report=Get-Content "$capture/capture.json" -Raw|ConvertFrom-Json
 if($report.fixture -or $report.timed_out -or -not $report.capture_ready -or -not $report.ready -or $report.diagnosis_error){throw 'Recovered project is not actually ready'}
 & "$root/current/bin/ffmpeg.exe" -hide_banner -loglevel error -n -i "$capture/capture.ppm" -frames:v 1 "$capture/capture.png"
 if($LASTEXITCODE -ne 0){throw 'Recovered GUI capture conversion failed'}
}
if(Test-Path "$trialPath/before-project-hashes.json"){
 foreach($property in $preserved.PSObject.Properties){if((Get-FileHash -LiteralPath $property.Name).Hash -ne $property.Value){throw "User data changed after recovery GUI: $($property.Name)"}}
}
@{recovered_version='0.1.1';authenticated_retained_package=$package;verification=($verification|Out-String);installed_manifest_files=@($manifest.sha256.PSObject.Properties).Count;extracted_files=$files;extracted_logical_bytes=$bytes;prior_current_preserved=$quarantine;source='retained authenticated full A package, independent of helper backup';automatic_rollback=$false}|ConvertTo-Json|Set-Content "$trialPath/manual-recovery.json"
Get-Content "$trialPath/manual-recovery.json"
