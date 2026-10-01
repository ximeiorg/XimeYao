# Rebuild Xime and test with the real install effect (MSIX development registration).
#
# Flow mirrors msix-bundle.ps1 -Register, so testing happens against the exact
# layout that gets installed:
#   1. build release (windows_subsystem -> no console window)
#   2. stage install layout to target\msix-pkg (binaries + rime.dll + data + user-data + resources)
#   3. Add-AppxPackage -Register (loose-file development registration = install effect)
#   4. start winxime-server.exe from the staged layout (same as MSI's StartServer action)
#
# Notes:
#   - Auto-elevates via UAC (the server self-registers the TSF DLL, which writes HKLM).
#   - target\msix-pkg must stay on disk: the registered package points at that folder
#     (it plays the role of C:\Program Files\WindowsApps for a real install).
#   - User data lives in %APPDATA%\Xime and persists across rebuilds, like a real install.
#   - Logs: %TEMP%\winxime\*.log
#
# 用法：
#   .\rebuild.ps1                        # CPU 包（默认，语音识别走 CPU 推理）
#   .\rebuild.ps1 -Gpu                   # CUDA 包（GPU 加速；需 CUDA 13 运行库的 cuBLAS）
#   .\rebuild.ps1 -Gpu -SherpaLibDir <path>
# CUDA 包额外要求：机器上有 CUDA 13 运行库（cublas64_13.dll + cublasLt64_13.dll）
# 与 cuDNN 9；缺库时 server 会**自动回退 CPU**（日志里有 warn），不会崩。
# 详见 DECISIONS.md「CUDA 包」相关条目。

param(
    # 构建 CUDA 加速包（语音识别走 GPU）。与 CPU 包**分目标目录**构建：
    # 两种 sherpa-onnx 预编译包里的 onnxruntime.dll 同名不同物
    # （CPU 16.97MB / CUDA 15.53MB），同一目录会互相覆盖、链错运行库。
    [switch]$Gpu,
    # CUDA 预编译库目录（内含 onnxruntime_providers_cuda.dll）。
    # 留空则在 $HOME\.cargo\sherpa-archives 下自动找带 cuda 的那个包。
    [string]$SherpaLibDir = ""
)

$ErrorActionPreference = "Stop"

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    # 自动以管理员重启本脚本：UAC 确认后在新窗口继续执行（-NoExit 保持窗口显示输出）。
    # **参数要原样带过去**：只传 -File 的话提权后 $Gpu 变回默认值，`-Gpu` 会被
    # 静默丢掉（打出来的其实是 CPU 包，而用户以为在验 GPU）。
    try {
        $argList = @("-NoExit", "-ExecutionPolicy", "Bypass", "-File", "$PSCommandPath")
        if ($Gpu) { $argList += "-Gpu" }
        if ($SherpaLibDir -ne "") { $argList += @("-SherpaLibDir", $SherpaLibDir) }
        Start-Process powershell.exe -Verb RunAs -ArgumentList $argList | Out-Null
    } catch {
        Write-Host "UAC cancelled; run again to retry." -ForegroundColor Red
        exit 1
    }
    exit
}

# cargo 与 msix-bundle.ps1 都按仓库根目录解析相对路径；提权重启后 cwd 是 System32，统一锚定。
Set-Location -LiteralPath $PSScriptRoot

$packageDir = "$PSScriptRoot\target\msix-pkg"

Write-Host "Step 0: Clearing old logs..." -ForegroundColor Yellow
Remove-Item "$env:TEMP\winxime\*.log" -Force -ErrorAction SilentlyContinue

Write-Host "Step 1: Stopping server and setup..." -ForegroundColor Yellow
# 优雅退出：用上一次构建的 server /q（IPC shutdown）；都没有时跳过，稍后强制结束兜底。
foreach ($exe in @(
    "$packageDir\winxime-server.exe",
    "$PSScriptRoot\target\release\winxime-server.exe",
    "$PSScriptRoot\target\debug\winxime-server.exe"
)) {
    if (Test-Path $exe) {
        Start-Process -FilePath $exe -ArgumentList "/q" -Wait -ErrorAction SilentlyContinue
        break
    }
}
# setup 也要停：暂存目录被占用会导致复制失败。
# 注意：不能直接 Get-Process | Stop-Process——无匹配进程时管道为空，
# Stop-Process 的必选参数 Id 会进入交互式提示（-ErrorAction 压不住）。
$staleProcesses = Get-Process -Name "winxime-server", "winxime-setup" -ErrorAction SilentlyContinue
if ($staleProcesses) {
    $staleProcesses | Stop-Process -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Seconds 2

# CUDA 包：定位预编译库目录 → 独立目标目录 → 带 feature 构建。
$stageDir = "target\release"
if ($Gpu) {
    if ($SherpaLibDir -eq "") {
        # 与 .cargo\config.toml 的 SHERPA_ONNX_ARCHIVE_DIR 保持同一约定；
        # [env] 只对 cargo 生效，这里自己找（accel 包目录名里带 cuda）。
        $archiveRoot = "$HOME\.cargo\sherpa-archives"
        $candidate = Get-ChildItem $archiveRoot -Directory -Filter "*cuda*" -ErrorAction SilentlyContinue |
            ForEach-Object { Join-Path $_.FullName "lib" } |
            Where-Object { Test-Path (Join-Path $_ "onnxruntime_providers_cuda.dll") } |
            Select-Object -First 1
        $SherpaLibDir = $candidate
    }
    if ([string]::IsNullOrEmpty($SherpaLibDir) -or -not (Test-Path $SherpaLibDir)) {
        Write-Host "找不到 CUDA 版 sherpa-onnx 预编译库（含 onnxruntime_providers_cuda.dll 的 lib 目录）。" -ForegroundColor Red
        Write-Host "  显式指定： .\rebuild.ps1 -Gpu -SherpaLibDir <解压后的 ...-win-x64-cuda\lib>" -ForegroundColor Yellow
        Write-Host "  下载：https://github.com/k2-fsa/sherpa-onnx/releases (sherpa-onnx-v*-cuda-*-win-x64-cuda.tar.bz2)" -ForegroundColor Yellow
        exit 1
    }
    $SherpaLibDir = (Resolve-Path $SherpaLibDir).Path
    $env:SHERPA_ONNX_LIB_DIR = $SherpaLibDir
    $env:CARGO_TARGET_DIR = "target-cuda"
    $stageDir = "target-cuda\release"
    Write-Host "  CUDA 库目录: $SherpaLibDir" -ForegroundColor Gray
    Write-Host "  目标目录:    target-cuda（与 CPU 包隔离，onnxruntime.dll 同名）" -ForegroundColor Gray
}

Write-Host "Step 2: Building release$(if ($Gpu) { ' (CUDA / speech-cuda)' })..." -ForegroundColor Yellow
if ($Gpu) {
    cargo build --release --quiet --features speech-cuda
} else {
    cargo build --release --quiet
}
if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}

Write-Host "Step 3: Staging install layout + MSIX registration..." -ForegroundColor Yellow
& "$PSScriptRoot\msix-bundle.ps1" -Register -TargetDir $stageDir
if ($LASTEXITCODE -ne 0) {
    Write-Host "Staging/registration failed!" -ForegroundColor Red
    exit 1
}

Write-Host "Step 4: Starting server from the staged layout..." -ForegroundColor Yellow
Start-Process -FilePath "$packageDir\winxime-server.exe"
Start-Sleep -Seconds 3

if (Get-Process -Name "winxime-server" -ErrorAction SilentlyContinue) {
    Write-Host ""
    Write-Host "Done! Server is running from the installed layout (no console window)." -ForegroundColor Green
    Write-Host "  Package layout: $packageDir" -ForegroundColor White
    Write-Host "  Logs:           $env:TEMP\winxime\*.log" -ForegroundColor White
    Write-Host "Test input in Notepad or any application." -ForegroundColor White
    Write-Host "NOTE: TSF DLL 是进程内加载的，请关掉重开要测试的应用（旧进程仍用旧 DLL）" -ForegroundColor Yellow
} else {
    Write-Host "Server did not start; check logs at $env:TEMP\winxime" -ForegroundColor Red
    exit 1
}
