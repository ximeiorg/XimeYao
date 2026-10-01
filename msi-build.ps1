# MSI build script
#
# 用法：
#   .\msi-build.ps1                  # CPU 包（默认，target\release）
#   .\msi-build.ps1 -Gpu             # CUDA 包（speech-cuda feature + target-cuda\release）
#   .\msi-build.ps1 -Gpu -SherpaLibDir "C:\path\to\sherpa-cuda\lib"
#
# CUDA 包与 CPU 包**必须分开构建、分开分发**：两者的 onnxruntime.dll 同名不同体
# （CUDA 版 15.5MB / CPU 版 17MB），装到一个目录里会互相覆盖。CUDA 包只多带
# 一个 onnxruntime_providers_cuda.dll（~255MB）；cuBLAS/cuDNN 不打包（~1GB），
# 要求机器上已有 CUDA 13 运行库，缺了会自动回退 CPU（见 rebuild.ps1 的说明）。

param(
    [string]$Version = "",
    [switch]$Gpu,
    [string]$SherpaLibDir = ""
)

$ErrorActionPreference = "Stop"

# Add WiX v3.14 to PATH
$env:PATH += ";C:\Program Files (x86)\WiX Toolset v3.14\bin"

# CUDA 包：定位 sherpa-onnx 的 CUDA 版运行库目录（build.rs 会从这里取 DLL），
# 并把 cargo 目标目录切到 target-cuda（与 CPU 包产物隔离，不能混）。
$TargetDir = "target\release"
$WithCudaDlls = 0
if ($Gpu) {
    $WithCudaDlls = 1
    if ($SherpaLibDir -eq "") {
        $archives = Join-Path $HOME ".cargo\sherpa-archives"
        $candidate = Get-ChildItem -Path $archives -Directory -Filter "*cuda*" -ErrorAction SilentlyContinue |
            ForEach-Object { Join-Path $_.FullName "lib" } |
            Where-Object { Test-Path (Join-Path $_ "onnxruntime_providers_cuda.dll") } |
            Select-Object -First 1
        if ($candidate) { $SherpaLibDir = $candidate }
    }
    if ($SherpaLibDir -eq "" -or -not (Test-Path (Join-Path $SherpaLibDir "onnxruntime_providers_cuda.dll"))) {
        Write-Host "找不到 CUDA 版 sherpa-onnx 运行库（需要 onnxruntime_providers_cuda.dll）。" -ForegroundColor Red
        Write-Host "请先按 rebuild.ps1 的说明下载 CUDA 归档，或用 -SherpaLibDir 指定 lib 目录。" -ForegroundColor Red
        exit 1
    }
    $env:SHERPA_ONNX_LIB_DIR = $SherpaLibDir
    $env:CARGO_TARGET_DIR = "target-cuda"
    $TargetDir = "target-cuda\release"
    Write-Host "CUDA 包：运行库 $SherpaLibDir" -ForegroundColor Cyan
}

# Auto-detect version from Cargo.toml
if ($Version -eq "") {
    $cargoTomlContent = Get-Content "Cargo.toml" -Raw
    if ($cargoTomlContent -match 'version\s*=\s*"([^"]+)"') {
        $Version = $matches[1]
    } else {
        $Version = "0.1.0"
    }
}

Write-Host "Building XimeYao (曦码·曜) MSI v$Version..." -ForegroundColor Cyan

# CUDA 包用不同文件名：两个包同版本号，混在一起会分不清哪个是哪个。
$MsiName = "ximeyao-$Version-x86_64$(if ($Gpu) { '-cuda' })"

# 1. Build release
Write-Host "Step 1: Building release..." -ForegroundColor Yellow
if ($Gpu) {
    cargo build --release --quiet --features speech-cuda
} else {
    cargo build --release --quiet
}
if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}

# 1.5. Copy rime.dll from libximecore git dep to $TargetDir
Write-Host "Step 1.5: Copying rime.dll..." -ForegroundColor Yellow
$rimeDll = Join-Path $librimeRoot "dist\lib\rime.dll"
if (Test-Path $rimeDll) {
    Copy-Item $rimeDll "$TargetDir\rime.dll" -Force
    Write-Host "  rime.dll copied"
} else {
    Write-Warning "rime.dll not found at $rimeDll"
}

# 2. data/ 不再分发 librime 自带 minimal 示例（cangjie5 / luna_pinyin /
#    essay.txt 等会混入用户 rime 目录）；rime-wubi（user-data/）自包含。
if (Test-Path "$TargetDir\data") {
    Remove-Item "$TargetDir\data" -Recurse -Force
}
New-Item "$TargetDir\data" -ItemType Directory -Force | Out-Null

# 2.5. Copy user schema files (to user-data/, deployed to %APPDATA% on first run)
Write-Host "Step 2.5: Copying user schema files..." -ForegroundColor Yellow
if (Test-Path "$TargetDir\user-data") {
    Remove-Item "$TargetDir\user-data" -Recurse -Force
}

$files = Get-ChildItem -Path "rime-wubi" -Recurse -File | Where-Object {
    $dir = $_.DirectoryName
    $name = $_.Name
    -not ($dir -like "*\.git*") -and
    -not ($dir -like "*\.github*") -and
    -not ($dir -like "*imgs*") -and
    -not ($name -like "*.md") -and
    -not ($name -like ".gitignore") -and
    -not ($name -like "macOS-*") -and
    -not ($name -like "*.command") -and
    -not ($name -like "LICENSE") -and
    -not ($name -like "squirrel.custom.yaml") -and
    -not ($name -like "trime.custom.yaml")
}

foreach ($file in $files) {
    $relativePath = $file.FullName.Substring($PWD.Path.Length + "rime-wubi".Length + 2)
    $destPath = "$TargetDir\user-data\$relativePath"
    $destDir = Split-Path -Parent $destPath
    if (-not (Test-Path $destDir)) {
        New-Item $destDir -ItemType Directory -Force | Out-Null
    }
    Copy-Item $file.FullName $destPath -Force
}

# 3. Copy resources
Write-Host "Step 3: Copying resources..." -ForegroundColor Yellow
if (Test-Path "$TargetDir\resources") {
    Remove-Item "$TargetDir\resources" -Recurse -Force
}
Copy-Item "resources" "$TargetDir\resources" -Recurse

# 4. Harvest data, user-data, and resources
Write-Host "Step 4: Harvesting data and resources..." -ForegroundColor Yellow
heat dir "$TargetDir\data" -o "crates\winxime-server\wix\data.wxs" -dr DataFolder -cg DataFiles -var var.DataDir -sreg -srd -ag
heat dir "$TargetDir\user-data" -o "crates\winxime-server\wix\userdata.wxs" -dr UserDataFolder -cg UserDataFiles -var var.UserDataDir -sreg -srd -ag
heat dir "$TargetDir\resources" -o "crates\winxime-server\wix\resources.wxs" -dr ResourcesFolder -cg ResourcesFiles -var var.ResourcesDir -sreg -srd -ag

# 5. Compile with candle
Write-Host "Step 5: Compiling WiX sources..." -ForegroundColor Yellow
if (-not (Test-Path "target\wix")) {
    New-Item "target\wix" -ItemType Directory -Force | Out-Null
}

candle -arch x64 "crates\winxime-server\wix\main.wxs" "crates\winxime-server\wix\data.wxs" "crates\winxime-server\wix\userdata.wxs" "crates\winxime-server\wix\resources.wxs" `
    -ext WixUIExtension -ext WixUtilExtension `
    "-dCargoTargetBinDir=$TargetDir" `
    "-dDataDir=$TargetDir\data" `
    "-dUserDataDir=$TargetDir\user-data" `
    "-dResourcesDir=$TargetDir\resources" `
    "-dWithCudaDlls=$WithCudaDlls" `
    "-dVersion=$Version" `
    -out "target\wix\"

if ($LASTEXITCODE -ne 0) {
    Write-Host "Candle failed!" -ForegroundColor Red
    exit 1
}

# 6. Link with light
Write-Host "Step 6: Linking MSI..." -ForegroundColor Yellow
light "target\wix\main.wixobj" "target\wix\data.wixobj" "target\wix\userdata.wixobj" "target\wix\resources.wixobj" `
    -ext WixUIExtension -ext WixUtilExtension `
    -cultures:zh-CN `
    -loc "crates\winxime-server\wix\zh-cn.wxl" `
    -out "target\wix\$MsiName.msi"

if ($LASTEXITCODE -ne 0) {
    Write-Host "Light failed!" -ForegroundColor Red
    exit 1
}

# 7. Check result
$msiPath = "target\wix\$MsiName.msi"
if (Test-Path $msiPath) {
    $msi = Get-Item $msiPath
    Write-Host ""
    Write-Host "Success: $($msi.FullName)" -ForegroundColor Green
    Write-Host "Size: $([math]::Round($msi.Length / 1MB, 2)) MB" -ForegroundColor White
    Write-Host ""
    Write-Host "Install: msiexec /i $($msi.FullName)" -ForegroundColor Yellow
} else {
    Write-Host "MSI build failed!" -ForegroundColor Red
    exit 1
}