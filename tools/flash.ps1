param(
    [Parameter(Position = 0, Mandatory = $true)]
    [string] $Artifact,

    [switch] $ResolveOnly
)

$picotool = if ($env:PICOTOOL_PATH) {
    $env:PICOTOOL_PATH
} else {
    (Get-Command picotool -ErrorAction Stop).Source
}

if (-not (Test-Path -LiteralPath $Artifact -PathType Leaf)) {
    throw "Firmware artifact not found: $Artifact"
}

if ($ResolveOnly) {
    Write-Output $picotool
    exit 0
}

& $picotool load -u -v -x -t elf $Artifact
exit $LASTEXITCODE
