param([Parameter(Mandatory)][string]$Case)
. "$PSScriptRoot/common.ps1"
$plan = Read-QualificationPlan $Case
Assert-Prepared $Case
Set-QualificationEnvironment $plan
# No implicit retry: preserve partial evidence and require a fresh case after a failed build.
foreach ($name in @('A','B')) {
    if ((Test-Path "$Case/build-$name.process.json") -or (Test-Path "$Case/source-$name/dist")) { throw 'Build already attempted; use a fresh case' }
}
foreach ($variant in @(@{name='A';version=$plan.version_a}, @{name='B';version=$plan.version_b})) {
    $name = $variant.name
    Invoke-RecordedProcess -File (Get-Command cargo).Source -Arguments @('dev','build','--gui','--mcp','--updates','--offline','--ffmpeg',"$($plan.inputs.runtime)/bin/ffmpeg.exe",'--speech',$plan.inputs.runtime) `
        -Directory "$Case/source-$name" -Log "$Case/build-$name" -TimeoutSeconds 3500
    $bundle = "$Case/source-$name/dist/noh"
    $manifest = Get-Content "$bundle/manifest.json" -Raw | ConvertFrom-Json
    if ($manifest.build.package_version -ne $variant.version) { throw 'Packaged version mismatch' }
    $process = Get-Content "$Case/build-$name.process.json" -Raw | ConvertFrom-Json
    Write-Json @{exit=0;seconds=$process.seconds;bundle=$bundle;version=$variant.version} "$Case/build-$name-result.json"
}
