param(
    [string]$Version = "",
    [switch]$Sign,
    [switch]$Register,
    [switch]$InstallUnsigned,
    # 二进制来源目录。CUDA 包构建在独立目标目录（target-cuda，见 rebuild.ps1 -Gpu）：
    # 两种预编译包的 onnxruntime.dll 同名（CPU 16.97MB / CUDA 15.53MB），
    # 放同一目录会互相覆盖、链错运行库。
    [string]$TargetDir = "target\release"
)

$ErrorActionPreference = "Continue"

# 锚定仓库根目录（脚本可从任意 cwd 调用，例如被提权重启的 rebuild.ps1 调用时进程 cwd 是 system32）。
# 注意：Set-Location 只改 PowerShell 当前位置（cmdlet/外部 exe 用它）；
# [System.IO.File] 等 .NET API 读进程级 cwd，必须用 SetCurrentDirectory 单独设置。
Set-Location -LiteralPath $PSScriptRoot
[System.IO.Directory]::SetCurrentDirectory($PSScriptRoot)

# Auto-detect version from Cargo.toml
if ($Version -eq "") {
    $cargoTomlContent = Get-Content "Cargo.toml" -Raw
    if ($cargoTomlContent -match 'version\s*=\s*"([^"]+)"') {
        $Version = $matches[1]
    } else {
        $Version = "0.1.0"
    }
}

$parts = $Version.Split('.')
$msixVersion = "{0}.{1}.{2}.0" -f $parts[0], $parts[1], $parts[2]

Write-Host "Building XimeYao (曦码·曜) MSIX v$msixVersion..." -ForegroundColor Cyan

$packageDir = "target\msix-pkg"
# 清理历史让位目录（无占用即删；仍被映射中的旧 DLL 占用则静默跳过，重启后可清）
Get-ChildItem "target" -Directory -Filter "msix-pkg.old-*" -ErrorAction SilentlyContinue |
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
if (Test-Path $packageDir) {
    try {
        Remove-Item $packageDir -Recurse -Force -ErrorAction Stop
    } catch {
        # 旧包中的 winxime_tsf.dll 可能仍被正在使用输入法的宿主进程映射：
        # 映射中的文件不允许删除/覆盖，但允许改名——整目录改名让位后重建，
        # 新进程加载新文件，旧进程继续用映射中的旧映像，重启后可清。
        $aside = "$packageDir.old-$([DateTime]::Now.ToString('yyyyMMddHHmmss'))"
        if (Move-Item $packageDir $aside -Force -ErrorAction SilentlyContinue) {
            Write-Host "旧包目录被占用，已改名让位: $aside" -ForegroundColor Yellow
        } else {
            Write-Warning "旧包目录无法腾空，将直接在原目录覆盖暂存"
        }
    }
}

New-Item "$packageDir\assets" -ItemType Directory -Force | Out-Null
New-Item "$packageDir\data" -ItemType Directory -Force | Out-Null
New-Item "$packageDir\resources" -ItemType Directory -Force | Out-Null

# 1. Copy binaries
Write-Host "Step 1: Copying binaries..." -ForegroundColor Yellow
Copy-Item "$TargetDir\winxime-server.exe" $packageDir
Copy-Item "$TargetDir\winxime_tsf.dll" $packageDir
Copy-Item "$TargetDir\winxime-setup.exe" $packageDir
Copy-Item "$TargetDir\winxime-tsf-register.exe" $packageDir
$rimeDll = "$TargetDir\rime.dll"
if (Test-Path $rimeDll) {
    Copy-Item $rimeDll $packageDir
} else {
    Write-Warning "rime.dll not found at $TargetDir, copying from libximecore..."
    $json = (& cargo metadata --format-version 1 2>$null) | ConvertFrom-Json
    $pkg = $json.packages | Where-Object { $_.name -eq 'librime-sys2' }
    if (-not $pkg) { Write-Error "librime-sys2 not found"; exit 1 }
    $manifestPath = $pkg.manifest_path
    $libximecoreRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $manifestPath))
    $srcDll = Join-Path $libximecoreRoot "librime\dist\lib\rime.dll"
    if (-not (Test-Path $srcDll)) { Write-Error "rime.dll not found at $srcDll"; exit 1 }
    Copy-Item $srcDll $rimeDll -Force
    Copy-Item $rimeDll $packageDir
}

# 1.5. Copy speech (sherpa-onnx) runtime DLLs
#      winxime-server 的导入表直接引用 sherpa-onnx-c-api.dll（链接期就定下了），
#      少了它 server 根本起不来；onnxruntime.dll 是它的依赖，
#      providers_shared 由 onnxruntime 运行时按需加载。
#      CUDA 分包另外多一个 provider（`onnxruntime_providers_cuda.dll`，
#      CPU 包的目录里没有，有就带上——同一份脚本管两种包）。
#      不带 providers_tensorrt：它导入 nvinfer_10/nvonnxparser_10/cudnn64_9，
#      我们从不请求 TensorRT EP，带上只会多一份装不全的运行库依赖。
Write-Host "Step 1.5: Copying speech runtime DLLs..." -ForegroundColor Yellow
$mandatoryDlls = @("sherpa-onnx-c-api.dll", "onnxruntime.dll")
$optionalDlls = @(
    "onnxruntime_providers_shared.dll",
    "sherpa-onnx-cxx-api.dll",
    "onnxruntime_providers_cuda.dll"
)
foreach ($dll in $mandatoryDlls) {
    $srcDll = "$TargetDir\$dll"
    if (-not (Test-Path $srcDll)) { Write-Error "语音运行库缺失：$srcDll（sherpa-onnx 没构建成功？）"; exit 1 }
    Copy-Item $srcDll $packageDir
}
foreach ($dll in $optionalDlls) {
    $srcDll = "$TargetDir\$dll"
    if (Test-Path $srcDll) { Copy-Item $srcDll $packageDir }
}

# 2. data/ 不再分发 librime 自带 minimal 示例（librime\data\minimal：
#    cangjie5 / luna_pinyin / essay.txt 等示例方案与语料，会混入用户 rime
#    目录）。rime-wubi（user-data/）自包含全部所需文件（default.yaml /
#    symbols.yaml 均有）。清空旧暂存，防止历史残留被 ensure_rime_data 拷入。
Write-Host "Step 2: Clearing staged data (librime minimal samples no longer shipped)..." -ForegroundColor Yellow
Remove-Item "$packageDir\data\*" -Recurse -Force -ErrorAction SilentlyContinue

# 3. Copy rime-wubi data (to user-data/, deployed to %APPDATA% on first run)
Write-Host "Step 3: Copying rime-wubi data..." -ForegroundColor Yellow
New-Item "$packageDir\user-data" -ItemType Directory -Force | Out-Null
$rimeWubiFiles = Get-ChildItem -Path "rime-wubi" -Recurse -File | Where-Object {
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
foreach ($file in $rimeWubiFiles) {
    $relativePath = $file.FullName.Substring($PWD.Path.Length + "rime-wubi".Length + 2)
    $destPath = "$packageDir\user-data\$relativePath"
    $destDir = Split-Path -Parent $destPath
    if (-not (Test-Path $destDir)) { New-Item $destDir -ItemType Directory -Force | Out-Null }
    Copy-Item $file.FullName $destPath -Force
}

# 4. Copy resources
Write-Host "Step 4: Copying resources..." -ForegroundColor Yellow
Copy-Item "resources\*" "$packageDir\resources" -Recurse

# 5. Copy MSIX assets (logos, manifest)
Write-Host "Step 5: Copying MSIX assets..." -ForegroundColor Yellow
Copy-Item "crates\winxime-server\msix\assets\*" "$packageDir\assets"
$manifest = Get-Content "crates\winxime-server\msix\AppxManifest.xml" -Raw -Encoding UTF8
$manifest = $manifest.Replace('{{VERSION}}', $msixVersion)
$utf8Bom = New-Object System.Text.UTF8Encoding($true)
[System.IO.File]::WriteAllText("$packageDir\AppxManifest.xml", $manifest, $utf8Bom)

# Find MakeAppx.exe and SignTool.exe
$kitRoot = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
$makeAppx = Get-ChildItem "$kitRoot\*\x64\MakeAppx.exe" | Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
if (-not $makeAppx) { Write-Error "MakeAppx.exe not found"; exit 1 }
$signTool = Get-ChildItem "$kitRoot\*\x64\SignTool.exe" | Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName

if ($Register) {
    Write-Host "Registering for development..." -ForegroundColor Yellow
    # 停掉包内进程，避免移除旧注册时文件被占用（rebuild.ps1 已先停，这里是独立调用时的兜底）。
    # 注意：不能直接 Get-Process | Stop-Process——无匹配进程时管道为空，
    # Stop-Process 的必选参数 Id 会进入交互式提示（-ErrorAction 压不住）。
    $staleProcesses = Get-Process -Name "winxime-server", "winxime-setup" -ErrorAction SilentlyContinue
    if ($staleProcesses) {
        $staleProcesses | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 1
    }

    # 同版本重复注册会被 0x80073CFB 拒绝（ERROR_PACKAGE_ALREADY_INSTALLED，禁止重装已安装的包）。
    # 开发循环版本号不变，所以先按 manifest 的 Identity.Name 移除旧注册，再重新注册。
    # 注意：manifest 带默认 XML 命名空间，PowerShell 的 XML 适配器用 .Package.Identity 属性
    # 路径取不到节点（返回 null），必须用 local-name() 的命名空间无关 XPath。
    $identityNode = Select-Xml -Path "$packageDir\AppxManifest.xml" -XPath "//*[local-name()='Identity']" |
        Select-Object -First 1
    $identityName = $identityNode.Node.Name
    if ([string]::IsNullOrEmpty($identityName)) {
        Write-Host "无法从 AppxManifest.xml 解析 Identity.Name！" -ForegroundColor Red
        exit 1
    }
    Get-AppxPackage -Name $identityName -ErrorAction SilentlyContinue |
        Remove-AppxPackage -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1

    Add-AppxPackage -Register "$packageDir\AppxManifest.xml" -Verbose
    if (-not $?) {
        Write-Host "Appx registration failed!" -ForegroundColor Red
        exit 1
    }
    # 注意：-Register 是松散文件注册，注册的包内容直接指向 target\msix-pkg，
    # 不能删除该目录（相当于安装后的 WindowsApps 目录常驻磁盘）。
    Write-Host "Development registration complete! Package layout kept at $packageDir" -ForegroundColor Green
    return
}

# Create MSIX
Write-Host "Step 6: Creating MSIX package..." -ForegroundColor Yellow
if (-not (Test-Path "target\wix")) { New-Item "target\wix" -ItemType Directory -Force | Out-Null }
$msixPath = "target\wix\ximeyao-$Version-x86_64.msix"
& $makeAppx pack /d $packageDir /p $msixPath /l
if ($LASTEXITCODE -ne 0) { Write-Error "MakeAppx failed"; exit 1 }

# Install unsigned (for testing without signing)
if ($InstallUnsigned) {
    Write-Host "Installing unsigned MSIX (AllowUnsigned)..." -ForegroundColor Yellow
    Add-AppxPackage -AllowUnsigned -Path $msixPath -Verbose
    Remove-Item $packageDir -Recurse -Force
    Write-Host "Installation complete!" -ForegroundColor Green
    return
}

# Sign (optional)
if ($Sign) {
    Write-Host "Step 7: Signing MSIX..." -ForegroundColor Yellow
    $cert = Get-ChildItem "Cert:\CurrentUser\My" | Where-Object { $_.Subject -eq "CN=XimeOrg" } | Select-Object -First 1
    if (-not $cert) {
        $cert = New-SelfSignedCertificate -Type Custom -Subject "CN=XimeOrg" -KeyUsage DigitalSignature -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3") -CertStoreLocation "Cert:\CurrentUser\My" -NotAfter (Get-Date).AddYears(3)
        Write-Host "  Created new self-signed certificate" -ForegroundColor Yellow
    }

    $inRoot = Get-ChildItem "Cert:\CurrentUser\Root" | Where-Object { $_.Subject -eq "CN=XimeOrg" } | Select-Object -First 1
    if (-not $inRoot) {
        $certPath = Join-Path $env:TEMP "XimeOrg.cer"
        Export-Certificate -Cert $cert -FilePath $certPath -Type CERT | Out-Null
        Import-Certificate -FilePath $certPath -CertStoreLocation "Cert:\CurrentUser\Root" | Out-Null
        Remove-Item $certPath -Force
        Write-Host "  Certificate installed to Trusted Root" -ForegroundColor Yellow
    }

    if ($signTool) {
        & $signTool sign /fd SHA256 /a /s My /n "XimeOrg" $msixPath
        Write-Host "Signed with self-signed certificate" -ForegroundColor Green
    } else {
        Write-Warning "SignTool not found, skipping signing"
    }
}

# Result
Write-Host ""
Write-Host "Success: $((Get-Item $msixPath).FullName)" -ForegroundColor Green
Write-Host "Size: $([math]::Round((Get-Item $msixPath).Length / 1MB, 2)) MB" -ForegroundColor White

Remove-Item $packageDir -Recurse -Force
