@echo off
setlocal DisableDelayedExpansion
set "app_dir=%~dp0"
if not exist "%app_dir%.jisjtu-install" set "app_dir=%LOCALAPPDATA%\JIsjtu\"
if not exist "%app_dir%.jisjtu-install" (
    echo Run Install.cmd first. For a custom installation, run Setup-Campus.cmd in the installed folder.
    pause
    exit /b 1
)
pushd "%app_dir%" || exit /b 1
call :setup
set "setup_status=%errorlevel%"
popd
pause
exit /b %setup_status%

:setup
if exist ".venv\Scripts\python.exe" goto dependencies
for %%V in (3.12 3.11 3.10) do (
    py -%%V -c "import struct,sys;sys.exit(struct.calcsize('P') != 8)" >nul 2>&1
    if not errorlevel 1 (
        py -%%V -m venv .venv
        goto created
    )
)
python -c "import struct,sys;sys.exit(not (struct.calcsize('P') == 8 and (3,10) <= sys.version_info[:2] <= (3,12)))" >nul 2>&1
if not errorlevel 1 (
    python -m venv .venv
    goto created
)
where winget.exe >nul 2>&1 || goto needpython
echo Installing Python 3.12 for the current user with WinGet...
winget install --exact --id Python.Python.3.12 --scope user --source winget
if exist "%LOCALAPPDATA%\Programs\Python\Python312\python.exe" (
    "%LOCALAPPDATA%\Programs\Python\Python312\python.exe" -m venv .venv
    goto created
)
py -3.12 -m venv .venv
:created
if errorlevel 1 goto needpython
:dependencies
".venv\Scripts\python.exe" -m pip install -r requirements-sso.txt
if errorlevel 1 exit /b 1
".venv\Scripts\python.exe" -X utf8 -c "import websockets, ddddocr"
if errorlevel 1 exit /b 1
echo Campus dependencies are ready. Fill in your jAccount settings and run JIsjtu --sso-login.
exit /b 0

:needpython
echo Install 64-bit Python 3.12 with the Python launcher, then run Setup-Campus.cmd again.
exit /b 1
