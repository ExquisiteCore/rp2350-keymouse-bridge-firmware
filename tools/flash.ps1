param(
    [Parameter(Position = 0, Mandatory = $true)]
    [string] $Artifact,

    [switch] $ResolveOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Artifact -PathType Leaf)) {
    throw "Firmware artifact not found: $Artifact"
}

$picotool = if (Test-Path Env:PICOTOOL_PATH) {
    if ([string]::IsNullOrWhiteSpace($env:PICOTOOL_PATH)) {
        throw 'PICOTOOL_PATH must not be empty or whitespace.'
    }

    $picotoolItem = Get-Item -LiteralPath $env:PICOTOOL_PATH -ErrorAction Stop
    if (-not (Test-Path -LiteralPath $picotoolItem.FullName -PathType Leaf)) {
        throw "PICOTOOL_PATH is not an executable file: $($picotoolItem.FullName)"
    }

    $executableExtensions = @(
        ([Environment]::GetEnvironmentVariable('PATHEXT', 'Process') -split ';') |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_ }
    )
    $extension = [IO.Path]::GetExtension($picotoolItem.FullName)
    if ($executableExtensions.Count -eq 0 -or $extension -notin $executableExtensions) {
        throw "PICOTOOL_PATH is not an executable application: $($picotoolItem.FullName)"
    }

    (Get-Command $picotoolItem.FullName -CommandType Application -ErrorAction Stop).Source
} else {
    (Get-Command picotool -CommandType Application -ErrorAction Stop).Source
}

if ($ResolveOnly) {
    Write-Output $picotool
    exit 0
}

$global:LASTEXITCODE = $null
& $picotool load -u -v -x -t elf $Artifact
if ($null -eq $global:LASTEXITCODE) {
    throw 'picotool did not report an exit code.'
}
exit $global:LASTEXITCODE
