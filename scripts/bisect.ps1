$root='D:\user\Code\siyuan-pomodoro'; $s="$root\scripts"
$orig=Get-Content "$root\test-workspace\conf\conf.json.orig" -Raw | ConvertFrom-Json
$real=Get-Content C:\SiYuanData\conf\conf.json -Raw | ConvertFrom-Json
foreach ($k in 'system','bazaar','api','stat','onboarding','fileTree','editor','export','graph','flashcard','ocr','appearance','ai') {
    & $s\kernel-stop.ps1 | Out-Null; Start-Sleep 2
    $c=Get-Content "$root\test-workspace\conf\conf.json.orig" -Raw | ConvertFrom-Json
    $c.$k = $real.$k
    if ($k -eq 'ai') { $c.ai.mcp.servers = @() }
    $c | ConvertTo-Json -Depth 50 | Set-Content "$root\test-workspace\conf\conf.json" -Encoding utf8
    Remove-Item "$s\kernel-6807.log" -ErrorAction SilentlyContinue
    & $s\kernel-start.ps1 | Out-Null; Start-Sleep 18
    $r = (Select-String -Path "$s\kernel-6807.log" -Pattern 'kernel plugin' | Select -First 1).Line
    "$k => $($r.Substring(20,[Math]::Min(70,$r.Length-20)))" | Tee-Object -Append "$s\bisect.out"
    if ($r -notmatch 'disabled') { break }
}
& $s\kernel-stop.ps1 | Out-Null
Copy-Item "$root\test-workspace\conf\conf.json.orig" "$root\test-workspace\conf\conf.json" -Force
