Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$artifact = Join-Path $repoRoot 'target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware'
$distDirectory = Join-Path $repoRoot 'dist'
$output = Join-Path $distDirectory 'rp2350-keymouse-bridge-firmware.uf2'
$temporaryOutput = Join-Path $distDirectory 'rp2350-keymouse-bridge-firmware.tmp.uf2'

Push-Location $repoRoot
try {
    $global:LASTEXITCODE = $null
    & cargo build --release --locked
    if ($null -eq $global:LASTEXITCODE -or $global:LASTEXITCODE -ne 0) {
        throw "Cargo release build failed with exit code $global:LASTEXITCODE"
    }
} finally {
    Pop-Location
}

if (-not (Test-Path -LiteralPath $artifact -PathType Leaf)) {
    throw "Release ELF not found: $artifact"
}

$converter = if (Test-Path Env:ELF2UF2_PATH) {
    if ([string]::IsNullOrWhiteSpace($env:ELF2UF2_PATH)) {
        throw 'ELF2UF2_PATH must not be empty or whitespace.'
    }
    (Get-Command $env:ELF2UF2_PATH -CommandType Application -ErrorAction Stop).Source
} else {
    $command = Get-Command elf2uf2-rs -CommandType Application -ErrorAction SilentlyContinue
    if ($null -eq $command) {
        throw 'elf2uf2-rs was not found. Install it with: cargo install elf2uf2-rs --locked'
    }
    $command.Source
}

New-Item -ItemType Directory -Path $distDirectory -Force | Out-Null

try {
    $global:LASTEXITCODE = $null
    & $converter $artifact $temporaryOutput
    if ($null -eq $global:LASTEXITCODE -or $global:LASTEXITCODE -ne 0) {
        throw "elf2uf2-rs failed with exit code $global:LASTEXITCODE"
    }
    if (-not (Test-Path -LiteralPath $temporaryOutput -PathType Leaf)) {
        throw "UF2 converter did not create: $temporaryOutput"
    }

    $bytes = [IO.File]::ReadAllBytes($temporaryOutput)
    if ($bytes.Length -eq 0 -or ($bytes.Length % 512) -ne 0) {
        throw "UF2 length must be a nonzero multiple of 512 bytes: $($bytes.Length)"
    }

    $magicStart0 = [Convert]::ToUInt32('0A324655', 16)
    $magicStart1 = [Convert]::ToUInt32('9E5D5157', 16)
    $magicEnd = [Convert]::ToUInt32('0AB16F30', 16)
    $familyFlag = [Convert]::ToUInt32('00002000', 16)
    $rp2350ArmSecureFamily = [Convert]::ToUInt32('E48BFF59', 16)
    $familyBytes = [BitConverter]::GetBytes($rp2350ArmSecureFamily)
    $blockCount = [int]($bytes.Length / 512)

    for ($block = 0; $block -lt $blockCount; $block++) {
        $offset = $block * 512
        if ([BitConverter]::ToUInt32($bytes, $offset) -ne $magicStart0 -or
            [BitConverter]::ToUInt32($bytes, $offset + 4) -ne $magicStart1 -or
            [BitConverter]::ToUInt32($bytes, $offset + 508) -ne $magicEnd) {
            throw "Invalid UF2 magic in block $block"
        }

        $flags = [BitConverter]::ToUInt32($bytes, $offset + 8)
        $targetAddress = [BitConverter]::ToUInt32($bytes, $offset + 12)
        $payloadSize = [BitConverter]::ToUInt32($bytes, $offset + 16)
        $blockNumber = [BitConverter]::ToUInt32($bytes, $offset + 20)
        $declaredBlocks = [BitConverter]::ToUInt32($bytes, $offset + 24)
        if (($flags -band $familyFlag) -eq 0) {
            throw "UF2 family flag is missing in block $block"
        }
        if ($blockNumber -ne $block -or $declaredBlocks -ne $blockCount) {
            throw "Invalid UF2 block numbering in block $block"
        }
        if ($payloadSize -gt 476) {
            throw "Invalid UF2 payload size in block ${block}: $payloadSize"
        }
        if ($targetAddress -lt 0x10000000 -or
            ([uint64]$targetAddress + [uint64]$payloadSize) -gt 0x10400000) {
            throw ('UF2 block {0} targets outside RP2350 flash: 0x{1:X8}' -f $block, $targetAddress)
        }

        [Array]::Copy($familyBytes, 0, $bytes, $offset + 28, 4)
    }

    [IO.File]::WriteAllBytes($temporaryOutput, $bytes)
    Move-Item -LiteralPath $temporaryOutput -Destination $output -Force
} finally {
    if (Test-Path -LiteralPath $temporaryOutput -PathType Leaf) {
        Remove-Item -LiteralPath $temporaryOutput -Force
    }
}

$hash = Get-FileHash -LiteralPath $output -Algorithm SHA256
Write-Output "ELF: $artifact"
Write-Output "UF2: $output"
Write-Output ('UF2 family: 0xE48BFF59 (RP2350 ARM-S), blocks: {0}' -f $blockCount)
Write-Output "SHA256: $($hash.Hash)"
