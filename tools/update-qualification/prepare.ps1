param([Parameter(Mandatory)][string]$Case)
. "$PSScriptRoot/common.ps1"
$plan = Read-QualificationPlan $Case
foreach ($name in @('source-A','source-B','fixture-key.pk8','public-keys.json','runtime-fixtures','tools')) {
    if (Test-Path -LiteralPath "$Case/$name") { throw "Preparation output already exists: $name; use a fresh case" }
}
Push-Location $workspace
try {
    $paths = @(& git -c core.quotepath=false ls-files --cached --others --exclude-standard | Sort-Object -Unique)
    if ($LASTEXITCODE -ne 0) { throw 'Source enumeration failed' }
    $head = & git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Source revision unavailable' }
    $status = @(& git status --short)
    if ($LASTEXITCODE -ne 0) { throw 'Source status unavailable' }
} finally { Pop-Location }
$inventory = @()
foreach ($relative in $paths) {
    # Snapshot only build/package inputs and their documentation. Never copy Git, caches or arbitrary local files.
    if ($relative -notmatch '^(src/|tools/|tests/|assets/|locales/|docs/|\.cargo/|Cargo\.toml$|Cargo\.lock$|build\.rs$|README\.md$|LICENSE[^/]*$)') { continue }
    if ($relative -match '(?i)(\.pk8$|\.pem$|\.key$|private[-_]key)') { throw "Private key is a source input: $relative" }
    $source = Assert-ChildPath (Join-Path $workspace $relative) $workspace
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { continue } # tracked deletion
    $hash = (Get-FileHash -LiteralPath $source).Hash
    foreach ($variant in @('A','B')) {
        $destination = Assert-ChildPath "$Case/source-$variant/$relative" "$Case/source-$variant"
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))
        [IO.File]::Copy($source, $destination, $false)
        if ((Get-FileHash -LiteralPath $destination).Hash -ne $hash) { throw "Source changed during snapshot: $relative" }
    }
    $inventory += [ordered]@{path=$relative;sha256_source=$hash;sha256_A=$hash;sha256_B=$hash}
}
foreach ($variant in @('A','B')) {
    Set-SnapshotVersion "$Case/source-$variant" $plan."version_$($variant.ToLowerInvariant())"
    foreach ($item in $inventory | Where-Object { $_.path -in @('Cargo.toml','Cargo.lock') }) {
        $item["sha256_$variant"] = (Get-FileHash "$Case/source-$variant/$($item.path)").Hash
    }
}
foreach ($item in $inventory) {
    if ((Get-FileHash -LiteralPath "$workspace/$($item.path)").Hash -ne $item.sha256_source) { throw "Source changed during preparation: $($item.path)" }
}
Write-Json $inventory "$Case/source-inventory.json"
Write-Json @{head=$head;working_tree=$status;captured_utc=[DateTime]::UtcNow.ToString('o')} "$Case/source-origin.json"
Set-QualificationEnvironment $plan
Invoke-RecordedProcess -File (Get-Command cargo).Source -Arguments @('build','--locked','--offline','--profile','qa','--features','updates','--example','update-release') `
    -Directory "$Case/source-A" -Log "$Case/build-verifier" -TimeoutSeconds 1500
[void][IO.Directory]::CreateDirectory("$Case/tools")
$tool = "$Case/tools/update-release.exe"
[IO.File]::Copy("$workspace/target/qa/examples/update-release.exe", $tool, $false)
Invoke-RecordedProcess -File $tool -Arguments @('keygen','--private-key',"$Case/fixture-key.pk8",'--public-keys',"$Case/public-keys.json",'--key-id','local-qualification') `
    -Directory $Case -Log "$Case/keygen"
[void][IO.Directory]::CreateDirectory("$Case/runtime-fixtures")
$ffmpeg = "$($plan.inputs.runtime)/bin/ffmpeg.exe"
Invoke-RecordedProcess -File $ffmpeg -Arguments @('-hide_banner','-loglevel','error','-n','-f','lavfi','-i','color=c=blue:s=320x180:r=25','-t','2','-c:v','libx264','-pix_fmt','yuv420p',"$Case/runtime-fixtures/video.mp4") -Directory $Case -Log "$Case/fixture-video"
Invoke-RecordedProcess -File $ffmpeg -Arguments @('-hide_banner','-loglevel','error','-n','-f','lavfi','-i','sine=frequency=440:sample_rate=48000','-t','2','-ac','2','-c:a','pcm_s16le',"$Case/runtime-fixtures/soundtrack.wav") -Directory $Case -Log "$Case/fixture-audio"
Write-Json @{passed=$true;source_files=$inventory.Count;verifier_sha256=(Get-FileHash $tool).Hash;public_keys_sha256=(Get-FileHash "$Case/public-keys.json").Hash} "$Case/prepared.json"
