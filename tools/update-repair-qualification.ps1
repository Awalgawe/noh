# Controlled native recovery acceptance; does not publish or repair ordinary installs.
param([Parameter(Mandatory)][string]$Case,[Parameter(Mandatory)][string]$Run,
      [Parameter(Mandatory)][Diagnostics.Process]$CompletedGuardian)
$ErrorActionPreference='Stop'
$workspace=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$allowed=(Resolve-Path "$workspace/.mcp-dev/update-feasibility").Path
$Case=(Resolve-Path -LiteralPath $Case).Path
$Run=(Resolve-Path -LiteralPath $Run).Path
if(-not $Case.StartsWith($allowed+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase) -or
   -not $Run.StartsWith($Case+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Recovery case/run outside qualification workspace'}
$CompletedGuardian.Refresh()
if(-not $CompletedGuardian.HasExited -or $CompletedGuardian.ExitCode -ne 0){throw 'Known guardian has not completed successfully'}
# This synchronous harness starts no new updater actor. Unknown processes are
# refused rather than killed; scans supplement, not replace, this precondition.
foreach($process in Get-Process){
 if($process.ProcessName -in @('noh-update-guard','Update','update_x64','update-measure')){
  $process.Refresh()
  if(-not $process.HasExited){throw "Possible updater owner remains: $($process.Id)"}
 }
}
$root=(Resolve-Path "$Run/installation").Path
foreach($process in Get-Process){if($process.Path -and $process.Path.StartsWith($root+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw "Installed process remains: $($process.Id)"}}
$plan=Get-Content "$Case/qualification-plan.json" -Raw|ConvertFrom-Json
$guiResult=Get-Content "$Run/result.json" -Raw|ConvertFrom-Json
if($guiResult.installed_version -ne $plan.version_b -or $guiResult.guardian_exit -ne 0 -or -not $guiResult.saved_project_preserved){throw 'Complete GUI acceptance must precede this recovery fixture'}
$tool=(Resolve-Path "$Run/external-noh-update-repair.exe").Path
$a=Get-Content "$Case/source-A/dist/noh/manifest.json" -Raw|ConvertFrom-Json
if((Get-FileHash -LiteralPath $tool).Hash.ToLowerInvariant() -ne $a.sha256.'bin/noh-update-repair.exe'){throw 'External repair binary differs from delivered A'}
$retained=(Get-Content "$Case/retained-A.json" -Raw|ConvertFrom-Json).retained_directory
$evidence=Join-Path $Run 'external-repair-qualification'
New-Item -ItemType Directory -Path $evidence|Out-Null
$before=Get-Content "$Run/project-before-hashes.json" -Raw|ConvertFrom-Json
$languageBefore=[IO.File]::ReadAllText("$Run/profile/noh/language")
$sentinelBefore=(Get-FileHash -LiteralPath "$Run/user-data-sentinel.txt").Hash
function Assert-Data {
 foreach($item in $before.PSObject.Properties){if((Get-FileHash -LiteralPath $item.Name).Hash -ne $item.Value){throw 'Saved project/media changed'}}
 if([IO.File]::ReadAllText("$Run/profile/noh/language") -ne $languageBefore -or (Get-FileHash -LiteralPath "$Run/user-data-sentinel.txt").Hash -ne $sentinelBefore){throw 'Preferences/sentinel changed'}
}
# Exercise exact delivered tool's refusal paths before making current absent.
$envelope=Get-Content "$retained/envelope.json" -Raw|ConvertFrom-Json
$badSignature=Join-Path $evidence 'invalid-signature'
New-Item -ItemType Directory -Path $badSignature|Out-Null
$envelope.signature='AAAA'
$envelope|ConvertTo-Json -Compress|Set-Content "$badSignature/envelope.json" -Encoding utf8NoBOM
& $tool --root $root --retained-archive $badSignature --version $plan.version_a --controlled-manual-restore *> "$evidence/invalid-signature.log"
if($LASTEXITCODE -eq 0){throw 'Invalid signature accepted'}
if(-not (Select-String -LiteralPath "$evidence/invalid-signature.log" -SimpleMatch 'Update signature is not trusted' -Quiet)){throw 'Signature refusal failed for an unexpected reason'}
$badPackage=Join-Path $evidence 'corrupt-package'
New-Item -ItemType Directory -Path $badPackage|Out-Null
Copy-Item -LiteralPath "$retained/envelope.json" -Destination "$badPackage/envelope.json"
$packageName="$($plan.package_id)-$($plan.version_a)-win-x64-stable-full.nupkg"
[IO.File]::WriteAllText((Join-Path $badPackage $packageName),'corrupted retained bytes')
& $tool --root $root --retained-archive $badPackage --version $plan.version_a --controlled-manual-restore *> "$evidence/corrupt-package.log"
if($LASTEXITCODE -eq 0){throw 'Corrupted package accepted'}
if(-not (Select-String -LiteralPath "$evidence/corrupt-package.log" -SimpleMatch 'Update package size or digest does not match' -Quiet)){throw 'Corrupt package refusal failed for an unexpected reason'}
Assert-Data
$b=Get-Content "$Case/source-B/dist/noh/manifest.json" -Raw|ConvertFrom-Json
foreach($item in $b.sha256.PSObject.Properties){if((Get-FileHash -LiteralPath (Join-Path "$root/current" $item.Name)).Hash.ToLowerInvariant() -ne $item.Value){throw 'B application changed during refusal checks'}}
$broken=Join-Path $root ('current-before-manual-repair-'+[guid]::NewGuid().ToString('N'))
# Both absolute paths are verified inside this disposable installation before move.
if(-not $broken.StartsWith($root+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Interrupted tree destination outside installation'}
Move-Item -LiteralPath "$root/current" -Destination $broken
if(Test-Path "$root/current"){throw 'Absent-current simulation failed'}
$timer=[Diagnostics.Stopwatch]::StartNew()
& $tool --root $root --retained-archive $retained --version $plan.version_a --controlled-manual-restore *> "$evidence/tool.log"
$toolExit=$LASTEXITCODE
$timer.Stop()
if($toolExit -ne 0){throw 'External restoration failed; all evidence preserved'}
$restored=Get-Content "$evidence/tool.log" -Raw|ConvertFrom-Json
if(-not $restored.restored_files_verified -or $restored.version -ne $plan.version_a){throw 'Wrong restoration result'}
foreach($item in $a.sha256.PSObject.Properties){if((Get-FileHash -LiteralPath (Join-Path "$root/current" $item.Name)).Hash.ToLowerInvariant() -ne $item.Value){throw 'Restored package manifest mismatch'}}
$identity=& "$root/current/bin/noh-cli.exe" --build-info|ConvertFrom-Json
if($LASTEXITCODE -ne 0 -or $identity.build_fingerprint -ne $a.build.build_fingerprint){throw 'Restored CLI did not start with A identity'}
$capture=Join-Path $evidence 'A-project-capture'
New-Item -ItemType Directory -Path $capture|Out-Null
$app=(Resolve-Path "$root/current/noh.exe").Path
$psi=[Diagnostics.ProcessStartInfo]::new($app)
$psi.UseShellExecute=$false;$psi.CreateNoWindow=$true;$psi.RedirectStandardOutput=$true;$psi.RedirectStandardError=$true
foreach($key in @($psi.Environment.Keys|Where-Object {$_ -like 'NOH_CAPTURE_*' -or $_ -like 'NOH_UPDATE_QUALIFICATION_*' -or $_ -eq 'NOH_UPDATE_TOKEN' -or $_ -eq 'NOH_LANGUAGE'})){[void]$psi.Environment.Remove($key)}
$psi.Environment['APPDATA']="$Run/profile";$psi.Environment['LOCALAPPDATA']="$Run/profile"
$psi.Environment['TEMP']="$Run/profile";$psi.Environment['TMP']="$Run/profile"
$psi.Environment['NOH_CAPTURE_UI']="$capture/capture.ppm";$psi.Environment['NOH_CAPTURE_PROJECT']="$Run/project-data/project.json"
$psi.Environment['NOH_FFMPEG']="$root/current/bin/ffmpeg.exe"
$psi.Environment['NOH_CAPTURE_WIDTH']='980';$psi.Environment['NOH_CAPTURE_HEIGHT']='850';$psi.Environment['NOH_CAPTURE_SCALE']='1';$psi.Environment['NOH_CAPTURE_THEME']='dark'
$gui=[Diagnostics.Process]::Start($psi)
$stdout=$gui.StandardOutput.ReadToEndAsync();$stderr=$gui.StandardError.ReadToEndAsync()
if(-not $gui.WaitForExit(30000)){
 if($gui.Path -ne $app){throw 'Refusing to stop unrelated capture process'}
 $gui.Kill();[void]$gui.WaitForExit(5000);throw 'Restored project capture deadline'
}
$stdout.GetAwaiter().GetResult()|Set-Content "$capture/stdout.log"
$stderr.GetAwaiter().GetResult()|Set-Content "$capture/stderr.log"
if($gui.ExitCode -ne 0){throw 'Restored A GUI failed'}
$report=Get-Content "$capture/capture.json" -Raw|ConvertFrom-Json
if($report.fixture -or $report.update_layout_fixture -or $report.timed_out -or -not $report.capture_ready -or -not $report.ready -or $report.diagnosis_error){throw 'Restored saved project was not ready'}
& "$root/current/bin/ffmpeg.exe" -hide_banner -loglevel error -n -i "$capture/capture.ppm" -frames:v 1 "$capture/capture.png"
if($LASTEXITCODE -ne 0){throw 'Restored capture conversion failed'}
Assert-Data
foreach($item in $a.sha256.PSObject.Properties){if((Get-FileHash -LiteralPath (Join-Path "$root/current" $item.Name)).Hash.ToLowerInvariant() -ne $item.Value){throw 'Restored A application changed after GUI closure'}}
if(-not (Test-Path $broken)){throw 'Interrupted B tree was not preserved'}
@{actual_external_tool=$tool;tool_exit=$toolExit;guardian_exit=$CompletedGuardian.ExitCode;simulated_absent_current=$true;preserved_b=$broken;restored_version=$plan.version_a;cli_fingerprint=$identity.build_fingerprint;gui_exit=$gui.ExitCode;saved_project_ready=$true;data_preserved=$true;invalid_signature_refused=$true;corrupt_package_refused=$true;seconds=$timer.Elapsed.TotalSeconds;tool_result=$restored;power_loss_qualified=$false}|ConvertTo-Json -Depth 7|Set-Content "$evidence/result.json"
Write-Output 'Actual external repair, A CLI and saved-project GUI qualified; data preserved'
