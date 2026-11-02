# 只停止由 kernel-start.ps1 启动的内核（按 PID），不会碰桌面端自带的内核。
param([int]$Port = 6807)
$pidFile = Join-Path $PSScriptRoot "kernel-$Port.pid"
if (Test-Path $pidFile) {
    Stop-Process -Id (Get-Content $pidFile) -Force -ErrorAction SilentlyContinue
    Remove-Item $pidFile
    Write-Host "已停止"
} else { Write-Host "没有记录的 PID" }
