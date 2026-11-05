param(
    [string]$ToolchainBin = "$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin"
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$cargoPath = Join-Path $ToolchainBin 'cargo.exe'
$rustcPath = Join-Path $ToolchainBin 'rustc.exe'
if (!(Test-Path -LiteralPath $cargoPath) -or !(Test-Path -LiteralPath $rustcPath)) {
    throw '找不到 MSVC Rust 工具链，请通过 -ToolchainBin 指定其 bin 目录。不要使用 LLVM/GNU 工具链发布。'
}
$rustInfo = & $rustcPath -vV
if ($LASTEXITCODE -ne 0 -or !($rustInfo -match '^host: x86_64-pc-windows-msvc$')) {
    throw '发布必须使用 host 为 x86_64-pc-windows-msvc 的 Rust 工具链。'
}

$savedPath = $env:PATH
$savedRustc = $env:RUSTC
$savedTemp = $env:TEMP
$savedTmp = $env:TMP
Push-Location (Join-Path $projectRoot 'app\src-tauri')
try {
    $env:PATH = "$ToolchainBin;$savedPath"
    $env:RUSTC = $rustcPath
    $buildTemp = Join-Path (Get-Location) 'target\build-tmp'
    New-Item -ItemType Directory -Force -Path $buildTemp | Out-Null
    $env:TEMP = $buildTemp
    $env:TMP = $buildTemp
    & $cargoPath build --release --locked --offline --target x86_64-pc-windows-msvc --features tauri/custom-protocol
    if ($LASTEXITCODE -ne 0) { throw 'MSVC release 编译失败，停止打包。' }

    $sourceExe = 'target\x86_64-pc-windows-msvc\release\siyuan-pomodoro.exe'
    $dumpbinPattern = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\*\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe"
    $dumpbin = Get-ChildItem -Path $dumpbinPattern -File | Sort-Object FullName -Descending | Select-Object -First 1
    if (!$dumpbin) { throw '找不到 Visual Studio 2022 dumpbin，无法检查发布依赖。' }
    $dependencies = & $dumpbin.FullName /dependents $sourceExe
    if ($LASTEXITCODE -ne 0) { throw '读取 exe 依赖失败，停止发布。' }
    if ($dependencies -match 'WebView2Loader\.dll') { throw '产物依赖外置 WebView2Loader.dll，拒绝发布。' }
    Write-Output $dependencies

    # 只发布 MSVC 目录的产物，不读取 target/release 下可能残留的 GNU 构建。
    $version = (Get-Content -LiteralPath 'tauri.conf.json' -Raw | ConvertFrom-Json).version
    $distDir = Join-Path $projectRoot 'dist'
    New-Item -ItemType Directory -Force -Path $distDir | Out-Null
    $exeName = "siyuan-pomodoro-v$version-windows-x64.exe"
    $outputPath = Join-Path $distDir $exeName
    Copy-Item -LiteralPath $sourceExe -Destination $outputPath
    $hashValue = (Get-FileHash -LiteralPath $outputPath -Algorithm SHA256).Hash
    $checksumPath = Join-Path $distDir "SHA256SUMS-v$version.txt"
    [System.IO.File]::WriteAllText($checksumPath, "$hashValue  $exeName`n", [System.Text.UTF8Encoding]::new($false))
    Write-Output "单 exe：$outputPath"
    Write-Output "SHA256：$hashValue"
} finally {
    Pop-Location
    $env:PATH = $savedPath
    $env:RUSTC = $savedRustc
    $env:TEMP = $savedTemp
    $env:TMP = $savedTmp
}
