# 单独启动思源内核（无界面）。默认指向测试工作空间，端口 6807，避免和桌面端的 6806 冲突。
param(
    [string]$Workspace = (Join-Path $PSScriptRoot '..\test-workspace'),
    [int]$Port = 6807,
    [string]$Resources = "$env:LOCALAPPDATA\Programs\SiYuan\resources"
)
$Workspace = (Resolve-Path $Workspace).Path
$pidFile = Join-Path $PSScriptRoot "kernel-$Port.pid"
$log = Join-Path $PSScriptRoot "kernel-$Port.log"
if ((Test-Path $pidFile) -and (Get-Process -Id (Get-Content $pidFile) -ErrorAction SilentlyContinue)) {
    Write-Host "已在运行，PID $(Get-Content $pidFile)"; return
}
$p = Start-Process -FilePath "$Resources\kernel\SiYuan-Kernel.exe" `
    -ArgumentList "serve", "--port=$Port", "--wd=`"$Resources`"", "--workspace=`"$Workspace`"" `
    -WindowStyle Hidden -PassThru -RedirectStandardOutput $log -RedirectStandardError "$log.err"
$p.Id | Set-Content $pidFile
Write-Host "已启动 PID $($p.Id)，端口 $Port，工作空间 $Workspace"
