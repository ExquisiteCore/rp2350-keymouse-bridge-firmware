Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$buildScript = Join-Path $repoRoot 'tools\build-release.ps1'
$firmwareElf = Join-Path $repoRoot 'target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware'
$firmwareUf2 = Join-Path $repoRoot 'dist\rp2350-keymouse-bridge-firmware.uf2'

if (-not (Test-Path -LiteralPath $buildScript -PathType Leaf)) {
    throw "Release build script is missing: $buildScript"
}

& $buildScript
if ($LASTEXITCODE -ne 0) {
    throw "Release build script failed with exit code $LASTEXITCODE"
}

if (-not (Test-Path -LiteralPath $firmwareElf -PathType Leaf)) {
    throw "Release ELF is missing: $firmwareElf"
}
if (-not (Test-Path -LiteralPath $firmwareUf2 -PathType Leaf)) {
    throw "Release UF2 is missing: $firmwareUf2"
}

$bytes = [IO.File]::ReadAllBytes($firmwareUf2)
if ($bytes.Length -eq 0 -or ($bytes.Length % 512) -ne 0) {
    throw "UF2 length must be a nonzero multiple of 512 bytes: $($bytes.Length)"
}

$magicStart0 = [Convert]::ToUInt32('0A324655', 16)
$magicStart1 = [Convert]::ToUInt32('9E5D5157', 16)
$magicEnd = [Convert]::ToUInt32('0AB16F30', 16)
$familyFlag = [Convert]::ToUInt32('00002000', 16)
$rp2350ArmSecureFamily = [Convert]::ToUInt32('E48BFF59', 16)
$blockCount = [int]($bytes.Length / 512)

for ($block = 0; $block -lt $blockCount; $block++) {
    $offset = $block * 512
    if ([BitConverter]::ToUInt32($bytes, $offset) -ne $magicStart0 -or
        [BitConverter]::ToUInt32($bytes, $offset + 4) -ne $magicStart1 -or
        [BitConverter]::ToUInt32($bytes, $offset + 508) -ne $magicEnd) {
        throw "Invalid UF2 magic in block $block"
    }

    $flags = [BitConverter]::ToUInt32($bytes, $offset + 8)
    $blockNumber = [BitConverter]::ToUInt32($bytes, $offset + 20)
    $declaredBlocks = [BitConverter]::ToUInt32($bytes, $offset + 24)
    $family = [BitConverter]::ToUInt32($bytes, $offset + 28)
    if (($flags -band $familyFlag) -eq 0) {
        throw "UF2 family flag is missing in block $block"
    }
    if ($blockNumber -ne $block -or $declaredBlocks -ne $blockCount) {
        throw "Invalid UF2 block numbering in block $block"
    }
    if ($family -ne $rp2350ArmSecureFamily) {
        throw ('Unexpected UF2 family in block {0}: 0x{1:X8}' -f $block, $family)
    }
}

$hash = Get-FileHash -LiteralPath $firmwareUf2 -Algorithm SHA256
Write-Output ('PASS: {0} blocks, RP2350 ARM-S, SHA256 {1}' -f $blockCount, $hash.Hash)
