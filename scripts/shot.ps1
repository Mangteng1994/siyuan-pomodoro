# 截取小工具某个窗口（按标题包含的文字匹配）保存成 png。用法: pwsh shot.ps1 <输出png> [标题关键字，默认 思源番茄钟]
param([string]$Out, [string]$Title = '思源番茄钟')
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices;
public class ShotWin {
  public delegate bool P(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] static extern bool EnumWindows(P p, IntPtr l);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public struct R { public int L, T, Rt, B; }
  public static int[] Find(uint pid, string title) { int[] res = null; EnumWindows((h, x) => { uint p; GetWindowThreadProcessId(h, out p); var s = new StringBuilder(200); GetWindowText(h, s, 200); R r; GetWindowRect(h, out r); if (p == pid && IsWindowVisible(h) && s.ToString().Contains(title) && r.Rt - r.L > 50 && (res == null || (r.Rt - r.L) < (res[2] - res[0]))) res = new[]{r.L, r.T, r.Rt, r.B}; return true; }, IntPtr.Zero); return res; }
}
'@
[ShotWin]::SetProcessDPIAware() | Out-Null
$p = (Get-Process siyuan-pomodoro).Id
$r = [ShotWin]::Find($p, $Title)
if (-not $r) { throw "没找到窗口：$Title" }
$w = $r[2] - $r[0]; $h = $r[3] - $r[1]
$bmp = New-Object System.Drawing.Bitmap $w, $h
[System.Drawing.Graphics]::FromImage($bmp).CopyFromScreen($r[0], $r[1], 0, 0, (New-Object System.Drawing.Size $w, $h))
$bmp.Save($Out)
"$Out ($w x $h)"
