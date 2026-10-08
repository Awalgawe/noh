# Real GUI/guardian/repair acceptance, promoted from the qualified local driver.
param([Parameter(Mandatory)][string]$Case)
. "$PSScriptRoot/common.ps1"
$plan = Read-QualificationPlan $Case
Assert-Prepared $Case
$packaged = Get-Content "$Case/packaged.json" -Raw | ConvertFrom-Json
if (-not $packaged.passed -or $packaged.inputs.Count -lt 8) { throw 'Complete package stage must precede acceptance' }
foreach ($item in $packaged.inputs) {
 $path = Assert-ChildPath "$Case/$($item.path)" $Case
 if ((Get-FileHash -LiteralPath $path).Hash -ne $item.sha256) { throw "Packaged input changed: $($item.path)" }
}
Set-QualificationEnvironment $plan
$run=Join-Path $Case ('acceptance-'+[guid]::NewGuid().ToString('N'))
$root=Join-Path $run 'installation'
New-Item -ItemType Directory -Path $root|Out-Null
$run|Set-Content "$Case/acceptance-current.txt"
function Assert-Bundle([string]$Directory,[string]$Manifest){
 $value=Get-Content $Manifest -Raw|ConvertFrom-Json
 foreach($item in $value.sha256.PSObject.Properties){
  if((Get-FileHash -LiteralPath (Join-Path $Directory $item.Name)).Hash.ToLowerInvariant() -ne $item.Value){throw "Bundle mismatch: $($item.Name)"}
 }
 return $value
}
New-Item -ItemType Directory -Path "$root/current"|Out-Null
$initialArchive=Get-Content "$Case/retained-A.json" -Raw|ConvertFrom-Json
$fullA="$Case/releases/$($plan.package_id)-$($plan.version_a)-win-x64-stable-full.nupkg"
$initialPackage=[IO.File]::Open($fullA,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
try {
& "$Case/tools/update-release.exe" verify --public-keys "$Case/public-keys.json" --envelope "$($initialArchive.retained_directory)/envelope.json" --package $fullA --package-id $plan.package_id --expected-version $plan.version_a *> "$run/verify-initial-A.log"
if($LASTEXITCODE -ne 0){throw 'Initial full A authentication failed'}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip=[IO.Compression.ZipArchive]::new($initialPackage,[IO.Compression.ZipArchiveMode]::Read,$true)
try {
 $members=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
 foreach($entry in $zip.Entries){
  if([Text.RegularExpressions.Regex]::IsMatch($entry.FullName,'.__symlink$')){throw 'Reserved symlink member'}
  $entryLeaf=$entry.FullName.Split('/')[-1]
  $engineLike=$entryLeaf.EndsWith('Squirrel.exe') -or [Text.RegularExpressions.Regex]::IsMatch($entry.FullName,'_ExecutionStub.exe$')
  if(-not $entry.FullName.StartsWith('lib/app/')){
   if($engineLike){throw 'Engine member outside supported app namespace'}
   continue
  }
  $relative=$entry.FullName.Substring(8)
  if(-not $relative){continue}
  if($relative.Contains('\') -or $relative.Contains(':') -or @($relative.TrimEnd('/').Split('/')|Where-Object {$_ -in @('','.', '..')}).Count){throw 'Invalid initial package member'}

  if((($entry.ExternalAttributes -shr 16) -band 61440) -eq 40960 -or ($entry.ExternalAttributes -band 1024)){throw 'Linked initial package member'}
  $leaf=$relative.TrimEnd('/').Split('/')[-1]
  $destination=Join-Path "$root/current" $relative
  if($leaf.EndsWith('Squirrel.exe') -and $relative -ne 'Squirrel.exe'){throw 'Unsupported updater variant'}
  if([Text.RegularExpressions.Regex]::IsMatch($relative,'_ExecutionStub.exe$') -and $relative -ne "$($plan.package_id)_ExecutionStub.exe"){throw 'Unsupported execution stub variant'}
  if($leaf -eq 'Squirrel.exe'){
   if($relative -ne 'Squirrel.exe'){throw 'Unsupported nested updater member'}
   $destination=Join-Path $root 'Update.exe'
  } elseif($leaf.EndsWith('_ExecutionStub.exe')){
   if($relative -ne "$($plan.package_id)_ExecutionStub.exe"){throw 'Unsupported execution stub identity or path'}
   $destination=Join-Path $root "$($plan.package_id).exe"
  }
  $destination=[IO.Path]::GetFullPath($destination)
  if(-not $members.Add($destination)){throw 'Duplicate initial installed destination'}
  if(-not $destination.StartsWith([IO.Path]::GetFullPath($root)+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Unconfined initial package member'}
  if($entry.FullName.EndsWith('/')){New-Item -ItemType Directory -Force $destination|Out-Null;continue}
  New-Item -ItemType Directory -Force ([IO.Path]::GetDirectoryName($destination))|Out-Null
  [IO.Compression.ZipFileExtensions]::ExtractToFile($entry,$destination,$false)
 }
} finally {$zip.Dispose()}
} finally {$initialPackage.Dispose()}
$a=Assert-Bundle "$root/current" "$Case/source-A/dist/noh/manifest.json"
New-Item -ItemType File -Path "$root/.portable"|Out-Null
if((Get-FileHash -LiteralPath "$root/Update.exe").Hash -ne (Get-FileHash -LiteralPath "$($plan.inputs.packager)/vendor/update_x64.exe").Hash){throw 'Initial signed updater differs from the pinned helper'}
$profile=Join-Path $run 'profile'
New-Item -ItemType Directory -Path "$profile/noh"|Out-Null
[IO.File]::WriteAllText("$profile/noh/language",'fr')
[IO.File]::WriteAllText("$run/user-data-sentinel.txt",'must survive the local GUI update')
$before=(Get-FileHash -LiteralPath "$run/user-data-sentinel.txt").Hash
$data=Join-Path $run 'project-data'
New-Item -ItemType Directory -Path $data|Out-Null
Copy-Item -LiteralPath "$Case/runtime-fixtures/video.mp4","$Case/runtime-fixtures/soundtrack.wav" -Destination $data
$project=Join-Path $data 'project.json'
@{montage=@{videos=@("$data/video.mp4");wav="$data/soundtrack.wav";output="$data/export.mp4";ffmpeg="$root/current/bin/ffmpeg.exe";fade_in=0.0;fade_out=0.0;partial_fades=$false;preview=$false;clip_audio=$false;force_encode=$false};short=$null;captions=$null}|ConvertTo-Json -Depth 8|Set-Content $project -Encoding utf8NoBOM
$projectHashes=@{}
foreach($file in @($project,"$data/video.mp4","$data/soundtrack.wav")){$projectHashes[$file]=(Get-FileHash -LiteralPath $file).Hash}
$projectHashes|ConvertTo-Json|Set-Content "$run/project-before-hashes.json"
if($plan.requires_external_repair){
 $externalRepair=Join-Path $run 'external-noh-update-repair.exe'
 Copy-Item -LiteralPath "$root/current/bin/noh-update-repair.exe" -Destination $externalRepair
 if((Get-FileHash -LiteralPath $externalRepair).Hash.ToLowerInvariant() -ne $a.sha256.'bin/noh-update-repair.exe'){throw 'External repair copy identity mismatch'}
 $repairIdentity=& $externalRepair --build-info|ConvertFrom-Json
 if($LASTEXITCODE -ne 0 -or $repairIdentity.build_fingerprint -ne $a.build.build_fingerprint){throw 'External repair build identity mismatch'}
 $repairIdentity|ConvertTo-Json -Depth 5|Set-Content "$run/external-repair-build-info.json"
}
$incoming=Get-Content "$Case/retained-B.json" -Raw|ConvertFrom-Json
$app=(Resolve-Path "$root/current/noh.exe").Path
$psi=[Diagnostics.ProcessStartInfo]::new($app)
$psi.WorkingDirectory="$root/current";$psi.UseShellExecute=$false;$psi.CreateNoWindow=$true
$psi.RedirectStandardOutput=$true;$psi.RedirectStandardError=$true
foreach($key in @($psi.Environment.Keys|Where-Object {$_ -like 'NOH_CAPTURE_*' -or $_ -like 'NOH_UPDATE_QUALIFICATION_*' -or $_ -eq 'NOH_UPDATE_PREVIOUS_ARCHIVE' -or $_ -eq 'NOH_UPDATE_TOKEN' -or $_ -eq 'NOH_LANGUAGE'})){[void]$psi.Environment.Remove($key)}
$psi.Environment['APPDATA']=$profile;$psi.Environment['LOCALAPPDATA']=$profile
$psi.Environment['TEMP']=$profile;$psi.Environment['TMP']=$profile
$psi.Environment['NOH_UPDATE_QUALIFICATION_ACTION']='import-install'
$psi.Environment['NOH_UPDATE_QUALIFICATION_FROM_VERSION']=$plan.version_a
$psi.Environment['NOH_UPDATE_QUALIFICATION_ARCHIVE']=$incoming.retained_directory
$timer=[Diagnostics.Stopwatch]::StartNew()
$existingStages=@(Get-ChildItem "$Case/desktop-downloads" -Directory -Filter 'noh-update-*'|Select-Object -ExpandProperty FullName)
$gui=[Diagnostics.Process]::Start($psi)
$stdout=$gui.StandardOutput.ReadToEndAsync();$stderr=$gui.StandardError.ReadToEndAsync()
$guardianHandle=$null
$observedGuardianPath=$null
$deadline=[DateTime]::UtcNow.AddSeconds(300)
while($true){
 if(-not $guardianHandle){
  $candidateStages=@(Get-ChildItem "$Case/desktop-downloads" -Directory -Filter 'noh-update-*'|Where-Object {$_.FullName -notin $existingStages -and (Test-Path "$($_.FullName)/transaction/ready")})
  foreach($stage in $candidateStages){
   $expectedGuardian=Join-Path $stage.FullName 'noh-update-guard.exe'
   $candidates=@(Get-Process -ErrorAction SilentlyContinue|Where-Object {$_.Path -eq $expectedGuardian})
   if($candidates.Count -eq 1){$guardianHandle=$candidates[0];[void]$guardianHandle.Handle;$observedGuardianPath=$guardianHandle.Path;break}
  }
 }
 if($gui.WaitForExit(100)){break}
 if([DateTime]::UtcNow -gt $deadline){throw "A did not finish handoff within 300 seconds; process $($gui.Id) left for inspection"}
}
$stdout.GetAwaiter().GetResult()|Set-Content "$run/A-stdout.log"
$stderr.GetAwaiter().GetResult()|Set-Content "$run/A-stderr.log"
if($gui.ExitCode -ne 0){throw 'A exited with failure'}
$stages=@(Get-ChildItem "$Case/desktop-downloads" -Directory -Filter 'noh-update-*'|Where-Object {$_.FullName -notin $existingStages -and (Test-Path "$($_.FullName)/transaction/commit")})
if($stages.Count -ne 1){throw 'Expected exactly one committed GUI-owned staging directory'}
$transaction=Join-Path $stages[0].FullName 'transaction'
if(-not $guardianHandle){
 $expectedGuardian=Join-Path $stages[0].FullName 'noh-update-guard.exe'
 $candidates=@(Get-Process -ErrorAction SilentlyContinue|Where-Object {$_.Path -eq $expectedGuardian})
 if($candidates.Count -eq 1){$guardianHandle=$candidates[0];[void]$guardianHandle.Handle;$observedGuardianPath=$guardianHandle.Path}
}
if(-not $guardianHandle){throw 'Guardian was never observed live; refusing to infer process completion'}
if($observedGuardianPath -ne (Join-Path $stages[0].FullName 'noh-update-guard.exe')){throw 'Observed guardian differs from committed owner'}
if(-not $guardianHandle.WaitForExit(660000)){throw 'Guardian process completion deadline; inputs preserved'}
if($guardianHandle.ExitCode -ne 0){throw 'Guardian process exit failed'}
if([IO.File]::ReadAllText("$transaction/completed") -ne 'result-flushed'){throw 'Wrong completion marker'}
$result=Get-Content "$transaction/result.json" -Raw|ConvertFrom-Json
if(-not $result.success -or $result.version -ne $plan.version_b -or -not $result.restart.launched){throw 'Installation/restart result failed'}
$restarted=Get-Process -Id $result.restart.pid
if($restarted.Path -ne $app){throw 'Restarted executable is not the managed GUI'}
if(-not $restarted.WaitForInputIdle(30000)){throw 'Restarted GUI did not enter an input-idle state'}
$windowDeadline=[DateTime]::UtcNow.AddSeconds(30)
do {
 $restarted.Refresh()
 if($restarted.HasExited){throw 'Restarted GUI exited before visible window'}
 if($restarted.MainWindowHandle -ne 0 -and $restarted.MainWindowTitle.StartsWith('NOH') -and $restarted.Responding){break}
 if([DateTime]::UtcNow -ge $windowDeadline){throw 'Restarted GUI visible window deadline'}
 [Threading.Thread]::Sleep(100)
} while($true)
$b=Assert-Bundle "$root/current" "$Case/source-B/dist/noh/manifest.json"
$identity=& "$root/current/bin/noh-cli.exe" --build-info|ConvertFrom-Json
if($LASTEXITCODE -ne 0 -or $identity.build_fingerprint -ne $b.build.build_fingerprint){throw 'Installed CLI identity mismatch'}
if((Get-FileHash -LiteralPath "$run/user-data-sentinel.txt").Hash -ne $before -or [IO.File]::ReadAllText("$profile/noh/language") -ne 'fr'){throw 'User data/settings changed'}
foreach($file in $projectHashes.Keys){if((Get-FileHash -LiteralPath $file).Hash -ne $projectHashes[$file]){throw 'Saved project/media changed'}}
$verifiedArchives=@{}
foreach($version in @($plan.version_a,$plan.version_b)){
 $name="$($plan.package_id)-$version-win-x64-stable-full.nupkg"
 foreach($archive in @(Get-ChildItem "$Case/desktop-downloads/retained" -Directory -Filter 'retained-*')){
  if(-not (Test-Path (Join-Path $archive.FullName $name))){continue}
  & "$Case/tools/update-release.exe" verify --public-keys "$Case/public-keys.json" --envelope "$($archive.FullName)/envelope.json" --package (Join-Path $archive.FullName $name) --package-id $plan.package_id --expected-version $version *> "$run/verify-retained-$version.log"
  if($LASTEXITCODE -ne 0){throw 'Retained archive authentication failed'}
  $verifiedArchives[$version]=$archive.FullName
 }
 if(-not $verifiedArchives.ContainsKey($version)){throw "No retained recovery archive for version $version"}
}
$timer.Stop()
$summary=[ordered]@{run=$run;transaction=$transaction;mode='local authenticated import';automated_consent=$true;github_download_exercised=$false;initial_gui_exit=$gui.ExitCode;guardian_observed_live=$true;guardian_exit=$guardianHandle.ExitCode;installed_version=$result.version;restart_pid=$result.restart.pid;restart_window_responsive=$true;gui_media_readiness='pending separate real-project capture';package_hashes_verified=@($b.sha256.PSObject.Properties).Count;build_fingerprint=$identity.build_fingerprint;user_data_preserved=$true;language_preserved=$true;seconds=$timer.Elapsed.TotalSeconds;guardian=$result}
$summary|ConvertTo-Json -Depth 8|Set-Content "$run/result.json"
$verifiedArchives|ConvertTo-Json|Set-Content "$run/retained-archives.json"
# Only close our verified managed GUI; the guardian intentionally detached it.
[void]$restarted.CloseMainWindow()
if(-not $restarted.WaitForExit(5000)){
 if($restarted.Path -ne $app){throw 'Refusing to stop an unrelated process'}
 $restarted.Kill();[void]$restarted.WaitForExit(5000)
}
$capture=Join-Path $run 'B-project-capture'
New-Item -ItemType Directory -Path $capture|Out-Null
$psi=[Diagnostics.ProcessStartInfo]::new($app)
$psi.UseShellExecute=$false;$psi.CreateNoWindow=$true;$psi.RedirectStandardOutput=$true;$psi.RedirectStandardError=$true
foreach($key in @($psi.Environment.Keys|Where-Object {$_ -like 'NOH_CAPTURE_*' -or $_ -like 'NOH_UPDATE_QUALIFICATION_*' -or $_ -eq 'NOH_UPDATE_TOKEN' -or $_ -eq 'NOH_LANGUAGE'})){[void]$psi.Environment.Remove($key)}
$psi.Environment['APPDATA']=$profile;$psi.Environment['LOCALAPPDATA']=$profile
$psi.Environment['TEMP']=$profile;$psi.Environment['TMP']=$profile
$psi.Environment['NOH_CAPTURE_UI']="$capture/capture.ppm";$psi.Environment['NOH_CAPTURE_PROJECT']=$project
$psi.Environment['NOH_FFMPEG']="$root/current/bin/ffmpeg.exe"
$psi.Environment['NOH_CAPTURE_WIDTH']='980';$psi.Environment['NOH_CAPTURE_HEIGHT']='850';$psi.Environment['NOH_CAPTURE_SCALE']='1';$psi.Environment['NOH_CAPTURE_THEME']='dark'
$captured=[Diagnostics.Process]::Start($psi)
$out=$captured.StandardOutput.ReadToEndAsync();$err=$captured.StandardError.ReadToEndAsync()
if(-not $captured.WaitForExit(30000)){
 if($captured.Path -ne $app){throw 'Refusing to stop an unrelated capture process'}
 $captured.Kill();throw 'Real B project capture deadline'
}
$out.GetAwaiter().GetResult()|Set-Content "$capture/stdout.log"
$err.GetAwaiter().GetResult()|Set-Content "$capture/stderr.log"
if($captured.ExitCode -ne 0){throw 'Real B project opening failed'}
$report=Get-Content "$capture/capture.json" -Raw|ConvertFrom-Json
if($report.fixture -or $report.update_layout_fixture -or $report.timed_out -or -not $report.capture_ready -or -not $report.ready -or $report.diagnosis_error){throw 'Real B project was not ready'}
& "$root/current/bin/ffmpeg.exe" -hide_banner -loglevel error -n -i "$capture/capture.ppm" -frames:v 1 "$capture/capture.png"
if($LASTEXITCODE -ne 0){throw 'Capture conversion failed'}
foreach($file in $projectHashes.Keys){if((Get-FileHash -LiteralPath $file).Hash -ne $projectHashes[$file]){throw 'Saved project/media changed after GUI closure'}}
if((Get-FileHash -LiteralPath "$run/user-data-sentinel.txt").Hash -ne $before -or [IO.File]::ReadAllText("$profile/noh/language") -ne 'fr'){throw 'User data/settings changed after GUI closure'}
$summary.gui_media_readiness='verified by separate real-project B capture'
$summary.saved_project_preserved=$true
$summary.project_capture="$capture/capture.json"
$summary|ConvertTo-Json -Depth 8|Set-Content "$run/result.json"
$summary|ConvertTo-Json -Depth 8

if($plan.requires_external_repair){
 & "$workspace/tools/update-repair-qualification.ps1" -Case $Case -Run $run -CompletedGuardian $guardianHandle
 if(-not $?){throw 'External repair qualification failed'}
 @{gui_cycle_passed=$true;external_repair_passed=$true;gui_result="$run/result.json";repair_result="$run/external-repair-qualification/result.json"}|ConvertTo-Json|Set-Content "$run/qualification-chain-result.json"
}
