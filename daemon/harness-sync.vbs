' harness-sync.vbs - run the session sync daemon with no console window.
' Set HARNESS_SYNC_PYTHONW to a pythonw.exe path to override the default.
' Launched at login from the Startup folder. Single instance: exits if the
' pid file points at a live python process running harness_sync.py.
Option Explicit
Dim sh, fso, home, hs, py, pidFile, wmi, procs, p, running
Set sh  = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
home = sh.ExpandEnvironmentStrings("%USERPROFILE%")
hs   = home & "\.local\share\harness-sync"
py   = sh.ExpandEnvironmentStrings("%HARNESS_SYNC_PYTHONW%")
If py = "%HARNESS_SYNC_PYTHONW%" Then py = "pythonw.exe"
running = False
Set wmi = GetObject("winmgmts:\\.\root\cimv2")
Set procs = wmi.ExecQuery("SELECT CommandLine FROM Win32_Process WHERE Name='pythonw.exe'")
For Each p In procs
  If Not IsNull(p.CommandLine) Then
    If InStr(p.CommandLine, "harness_sync.py") > 0 Then running = True
  End If
Next
If Not running Then
  sh.Run """" & py & """ """ & hs & "\harness_sync.py""", 0, False
End If
