@echo off
setlocal EnableExtensions

rem ===========================================================================
rem  Shadow-desktop build: put this file NEXT TO rustdesk.exe, then run it.
rem
rem  It starts rustdesk.exe inside the CURRENT, logged-on user's own session and
rem  keeps it starting there at every logon.
rem
rem  It deliberately does NOT install a Windows service. The hidden desktop is
rem  brought up in one place only - the windowless build's no-argument start
rem  (core_main.rs, the `headless && args.is_empty()` branch, which calls
rem  start_shadow_desktop()). A service runs `rustdesk.exe --service`, and that
rem  process starts its child as `"<exe>" --server` (platform\windows.rs,
rem  launch_server), which goes down the `--server` branch and never reaches
rem  start_shadow_desktop(). So a service would listen for connections but never
rem  offer the hidden desktop: RUSTDESK-SHADOW would be missing from the display
rem  list the client receives.
rem
rem  The mode also has to sit in the interactive window station (WinSta0) of a
rem  logged-on session, which is where this script puts it and where a logon-time
rem  start of the user's own process ends up by itself.
rem
rem  Everything it does is written to install-autostart.log next to itself.
rem
rem  Uninstall:
rem    powershell -Command "Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name RustDeskShadow"
rem    taskkill /F /IM rustdesk.exe
rem ===========================================================================

set "EXE=%~dp0rustdesk.exe"
set "LOG=%~dp0install-autostart.log"
set "VALUE=RustDeskShadow"
set "PS=powershell -NoProfile -ExecutionPolicy Bypass -Command"
rem The value has to hold a quoted path, and nesting quotes through cmd into PowerShell is
rem not worth the trouble: [char]34 is the quote.
set "SET_RUN=$q=[char]34; Set-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name '%VALUE%' -Value ($q + $env:RD_SHADOW_EXE + $q) -Type String"

>"%LOG%" echo === install-autostart %DATE% %TIME% ===
>>"%LOG%" echo script=%~f0
>>"%LOG%" echo exe=%EXE%
>>"%LOG%" echo user=%USERDOMAIN%\%USERNAME%

echo [*] Shadow desktop autostart installer
echo     exe  : "%EXE%"
echo     user : %USERDOMAIN%\%USERNAME%
echo.

if exist "%EXE%" goto :have_exe
>>"%LOG%" echo result=exe-missing
echo [x] rustdesk.exe was not found next to this script:
echo     "%EXE%"
echo     Copy this file into the folder that holds rustdesk.exe and run again.
echo.
pause
goto :eof

:have_exe

rem ---------- 1) a leftover service of this build cannot work ----------
rem Through WMI rather than `sc delete`: sc.exe is on some machines' Command Security
rem program blacklist, and a blocked command would leave the stale service behind.
for /f "delims=" %%s in ('%PS% "Get-CimInstance Win32_Service -Filter \"Name='RustDesk'\" -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Name" 2^>nul') do set "STALE=%%s"
if not defined STALE goto :no_service
>>"%LOG%" echo found-service-RustDesk
echo [*] A service named RustDesk is registered. It cannot bring the shadow
echo     desktop up - it only ever starts the server as `--server`. Removing it,
echo     so the logon start below is the only instance.
taskkill /F /IM rustdesk.exe >>"%LOG%" 2>&1
%PS% "Get-CimInstance Win32_Service -Filter \"Name='RustDesk'\" | Invoke-CimMethod -MethodName StopService | Out-Null; Start-Sleep -Seconds 2; Get-CimInstance Win32_Service -Filter \"Name='RustDesk'\" | Invoke-CimMethod -MethodName Delete | Out-Null" >>"%LOG%" 2>&1
>>"%LOG%" echo delete-service-exit-code=%errorlevel%

:no_service

rem ---------- 2) start it in this user's session, now and at every logon ----------
taskkill /F /IM rustdesk.exe >>"%LOG%" 2>&1
set "RD_SHADOW_EXE=%EXE%"
%PS% "%SET_RUN%" >>"%LOG%" 2>&1
set "REG_RC=%errorlevel%"
>>"%LOG%" echo set-run-exit-code=%REG_RC%
if "%REG_RC%"=="0" goto :registered
echo [x] Could not write the autostart entry (PowerShell exit code %REG_RC%).
echo     See "%LOG%".
pause
goto :eof

:registered
echo [*] Autostart registered: HKCU\Software\Microsoft\Windows\CurrentVersion\Run\%VALUE%
echo [*] Starting rustdesk.exe now, with no arguments (that is what brings the
echo     hidden desktop up)...
start "" "%EXE%"
ping -n 4 127.0.0.1 >nul

echo.
echo ================== how to verify ==================
echo  1) The hidden desktop exists as soon as the process is up. In its log:
echo       %%APPDATA%%\RustDesk\log\server\rustdesk_rCURRENT.log
echo     you should see, within a second or two of start:
echo       shadow desktop is up, frames at N fps
echo     If you instead see "shadow desktop is turned off", the option is set to
echo     a false value in %%APPDATA%%\RustDesk\config\RustDesk2.toml. It belongs in
echo     the [options] table; an unset option means on:
echo       [options]
echo       shadow-desktop = 'Y'
echo.
echo     On the controlling side, the display list must contain
echo       \\.\RUSTDESK-SHADOW
echo     as display 1, ahead of the real monitors.
echo.
echo  2) Dump the relevant lines later with:
echo       findstr /s /i "shadow desktop" "%%APPDATA%%\RustDesk\*.log"
echo.
echo [OK] Done. Uninstall with:
echo      powershell -Command "Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name %VALUE%"
echo      taskkill /F /IM rustdesk.exe
echo.
echo      Keep dylib_virtual_display.dll in the same folder as the exe.
echo      Log file: "%LOG%"
echo.
pause
goto :eof
