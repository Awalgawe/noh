param([Parameter(Mandatory)][string]$Case)
. "$PSScriptRoot/common.ps1"
$plan = Read-QualificationPlan $Case
Assert-Prepared $Case
Set-QualificationEnvironment $plan
$keys = "$Case/public-keys.json"
$private = Assert-ChildPath "$Case/fixture-key.pk8" $Case
$tool = "$Case/tools/update-release.exe"
$releases = Assert-ChildPath "$Case/releases" $Case
if (Test-Path $releases) { throw 'Packages already attempted; use a fresh case' }
[void][IO.Directory]::CreateDirectory($releases)
foreach ($variant in @(@{name='A';version=$plan.version_a}, @{name='B';version=$plan.version_b})) {
    $label = $variant.name
    $build = Get-Content "$Case/build-$label-result.json" -Raw | ConvertFrom-Json
    if ($build.exit -ne 0 -or $build.version -ne $variant.version -or $build.bundle -ne "$Case/source-$label/dist/noh") { throw 'Build did not pass or bundle path changed' }
    $manifest = Get-Content "$($build.bundle)/manifest.json" -Raw | ConvertFrom-Json
    foreach ($item in $manifest.sha256.PSObject.Properties) {
        $path = Assert-ChildPath "$($build.bundle)/$($item.Name)" $build.bundle
        if ((Get-FileHash $path).Hash -ne $item.Value) { throw "Bundle changed: $($item.Name)" }
    }
    $timer = [Diagnostics.Stopwatch]::StartNew()
    Invoke-RecordedProcess -File $plan.inputs.dotnet -Arguments @("$($plan.inputs.packager)/tools/net8.0/any/vpk.dll",'pack','--packId',$plan.package_id,'--packVersion',$variant.version,'--packDir',$build.bundle,'--mainExe','noh.exe','--runtime','win-x64','--channel','win-x64-stable','--delta','None','--noPortable','--skipVeloAppCheck','--outputDir',$releases) -Directory $Case -Log "$Case/pack-$label" -TimeoutSeconds 800
    $name = "$($plan.package_id)-$($variant.version)-win-x64-stable-full.nupkg"
    $package = Get-Item -LiteralPath (Join-Path $releases $name)
    $release = @{schema_version=1;package_id=$plan.package_id;version=$variant.version;channel='stable';notes='Local GUI acceptance, no publication';artifacts=@(@{target=@{os='windows';arch='x86_64'};kind='full';base_version=$null;file_name=$name;url="https://github.com/example/noh/releases/download/gui-local/$name";size=$package.Length;sha256=(Get-FileHash -LiteralPath $package.FullName).Hash.ToLowerInvariant()})}
    Write-Json $release "$Case/release-$label.json"
    Invoke-RecordedProcess -File $tool -Arguments @('sign','--private-key',$private,'--key-id','local-qualification','--release',"$Case/release-$label.json",'--packages',$releases,'--output',"$Case/envelope-$label.json") -Directory $Case -Log "$Case/sign-$label"
    $archive = if ($label -eq 'A') { "$Case/desktop-downloads/retained" } else { "$Case/import-archives" }
    Invoke-RecordedProcess -File $tool -Arguments @('retain','--public-keys',$keys,'--envelope',"$Case/envelope-$label.json",'--package',$package.FullName,'--archive',$archive,'--package-id',$plan.package_id,'--expected-version',$variant.version) -Directory $Case -Log "$Case/retain-$label"
    Copy-Item -LiteralPath "$Case/retain-$label.stdout.log" -Destination "$Case/retained-$label.json"
    Write-Json @{version=$variant.version;seconds=$timer.Elapsed.TotalSeconds;package_bytes=$package.Length;sha256=$release.artifacts[0].sha256} "$Case/pack-$label-result.json"
}
$packageInputs = @()
foreach ($label in @('A','B')) {
    $retained = (Get-Content "$Case/retained-$label.json" -Raw | ConvertFrom-Json).retained_directory
    [void](Assert-ChildPath $retained $Case)
    foreach ($path in @("$Case/retained-$label.json", "$Case/pack-$label-result.json") + @(Get-ChildItem -LiteralPath $retained -File | Select-Object -ExpandProperty FullName)) {
        $packageInputs += @{path=[IO.Path]::GetRelativePath($Case,$path);sha256=(Get-FileHash -LiteralPath $path).Hash}
    }
}
Write-Json @{passed=$true;inputs=$packageInputs} "$Case/packaged.json"
