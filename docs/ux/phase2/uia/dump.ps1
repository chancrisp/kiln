# Dump the UI Automation tree Kiln serves (UX Phase 2). Read-only: it sends
# no keystrokes and no mouse input. The tree walk is the Phase 1 audit's.
#
# Launch mode starts the release kiln-app with a scratch APPDATA and
# LOCALAPPDATA under %TEMP%, passes copies of the sample files, waits, dumps
# the tree and closes the window with WM_CLOSE:
#   dump.ps1 -Name 01-empty -Note "Empty window"
#   dump.ps1 -Name 02-list -Note "Three files listed" -WithFiles
# Attach mode dumps a Kiln that is already open, in whatever state it is in,
# and leaves it open:
#   dump.ps1 -Name 03-wheel -Note "Convert wheel open" -AttachPid 1234
param(
  [Parameter(Mandatory)][string]$Name,
  [string]$Note = "",
  [switch]$WithFiles,
  [int]$AttachPid = 0,
  [int]$WaitMs = 2500
)
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, WindowsBase
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class KilnW {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr c);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
"@
# Physical pixels, as Kiln reports them.
[void][KilnW]::SetProcessDpiAwarenessContext([IntPtr](-4))

$repo = "C:\kiln"
$outDir = Join-Path $repo "docs\ux\phase2\uia"
$exe = Join-Path $repo "target\release\kiln-app.exe"
$scratch = Join-Path $env:TEMP "kiln-uia-dump"

function Fmt($r) { if ($r.IsEmpty) { "(empty)" } else { "x={0} y={1} w={2} h={3}" -f [int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height } }

function Describe($e) {
  $c = $e.Current
  $pats = ($e.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName -replace 'PatternIdentifiers.Pattern','' }) -join ","
  $extra = ""
  if ($c.HelpText) { $extra += " | help=`"$($c.HelpText)`"" }
  if ($c.ItemStatus) { $extra += " | status=`"$($c.ItemStatus)`"" }
  $sel = $null
  if ($e.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$sel)) { $extra += " | selected=$($sel.Current.IsSelected)" }
  $val = $null
  if ($e.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$val)) { $extra += " | value=`"$($val.Current.Value)`"" }
  "name=`"{0}`" | type={1} | automationId=`"{2}`" | rect={3} | keyboardFocusable={4} | hasFocus={5} | enabled={6} | offscreen={7} | patterns=[{8}]{9}" -f `
    $c.Name, ($c.ControlType.ProgrammaticName -replace 'ControlType.',''), $c.AutomationId, (Fmt $c.BoundingRectangle), $c.IsKeyboardFocusable, $c.HasKeyboardFocus, $c.IsEnabled, $c.IsOffscreen, $pats, $extra
}

function Walk($e, $depth, $walker, $sb, $all) {
  [void]$sb.AppendLine(("  " * $depth) + "- " + (Describe $e))
  if ($null -ne $all) { [void]$all.Add($e) }
  $child = $walker.GetFirstChild($e)
  while ($child) { Walk $child ($depth + 1) $walker $sb $all; $child = $walker.GetNextSibling($child) }
}

function Dump($hwnd) {
  $AE = [System.Windows.Automation.AutomationElement]
  $root = $AE::FromHandle($hwnd)
  $sb = [System.Text.StringBuilder]::new()
  [void]$sb.AppendLine("Kiln UIA dump: $Name")
  [void]$sb.AppendLine("State: $Note")
  [void]$sb.AppendLine("Captured: $(Get-Date -Format s), kiln-app release build, $([int]([KilnW]::GetDpiForWindow($hwnd)*100/96))% scale, scratch APPDATA and LOCALAPPDATA")
  [void]$sb.AppendLine("")
  $all = [System.Collections.ArrayList]::new()
  foreach ($view in @(@("Raw view (everything UIA exposes)", [System.Windows.Automation.TreeWalker]::RawViewWalker, $null),
                      @("Control view (what Narrator walks)", [System.Windows.Automation.TreeWalker]::ControlViewWalker, $all),
                      @("Content view", [System.Windows.Automation.TreeWalker]::ContentViewWalker, $null))) {
    [void]$sb.AppendLine("== $($view[0]) ==")
    Walk $root 0 $view[1] $sb $view[2]
    [void]$sb.AppendLine("")
  }
  [void]$sb.AppendLine("== Focused element ==")
  try { [void]$sb.AppendLine((Describe $AE::FocusedElement)) } catch { [void]$sb.AppendLine("error: $_") }
  [void]$sb.AppendLine("")
  # Each element's centre should hit that element, or one inside it.
  [void]$sb.AppendLine("== ElementFromPoint at each element's centre ==")
  foreach ($e in $all) {
    $r = $e.Current.BoundingRectangle
    if ($r.IsEmpty -or $e.Current.IsOffscreen) { continue }
    $pt = [System.Windows.Point]::new($r.X + $r.Width / 2, $r.Y + $r.Height / 2)
    try {
      $hit = $AE::FromPoint($pt)
      [void]$sb.AppendLine("`"$($e.Current.Name)`" -> `"$($hit.Current.Name)`" ($($hit.Current.ControlType.ProgrammaticName -replace 'ControlType.',''))")
    } catch { [void]$sb.AppendLine("`"$($e.Current.Name)`" -> error: $_") }
  }
  [void]$sb.AppendLine("")
  $cr = New-Object KilnW+RECT; [void][KilnW]::GetClientRect($hwnd, [ref]$cr)
  [void]$sb.AppendLine("Client rect: $($cr.R - $cr.L) x $($cr.B - $cr.T) px")
  $path = Join-Path $outDir "$Name.txt"
  Set-Content -Path $path -Value $sb.ToString() -Encoding utf8
  "wrote $path"
}

if ($AttachPid -gt 0) {
  $p = Get-Process -Id $AttachPid
  if ($p.ProcessName -ne "kiln-app") { throw "pid $AttachPid is not Kiln" }
  Dump $p.MainWindowHandle
  return
}

# One Kiln at a time: a second launch hands its files to the open window.
if (Get-Process kiln-app -ErrorAction SilentlyContinue) { throw "Close Kiln before running this." }
$appdata = Join-Path $scratch "appdata"
$local = Join-Path $scratch "localappdata"
$inputs = Join-Path $scratch "inputs"
New-Item -ItemType Directory -Force $appdata, $local, $inputs | Out-Null
$files = @()
if ($WithFiles) {
  foreach ($f in @("harbour.png", "interview.mov", "glaze test.png")) {
    $src = Join-Path $repo "target\samples\$f"
    if (Test-Path $src) {
      Copy-Item $src $inputs -Force
      $files += Join-Path $inputs $f
    }
  }
}
$env:APPDATA = $appdata
$env:LOCALAPPDATA = $local
$argList = @($files | ForEach-Object { "`"$_`"" })
$p = if ($argList.Count) { Start-Process -FilePath $exe -ArgumentList $argList -PassThru } else { Start-Process -FilePath $exe -PassThru }
Start-Sleep -Milliseconds $WaitMs
$p.Refresh()
$h = $p.MainWindowHandle
if ($h -eq [IntPtr]::Zero) { throw "Kiln has no window." }
try { Dump $h } finally {
  # WM_CLOSE: no keystrokes.
  [void][KilnW]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
  if (-not $p.WaitForExit(5000)) { "Kiln did not close; it is still open (pid $($p.Id))." } else { "closed" }
}
