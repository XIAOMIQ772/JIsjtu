@echo off
setlocal DisableDelayedExpansion
pushd "%~dp0.." || exit /b 1
if /i "%~1"=="--config" goto config
if /i "%~1"=="--help" goto help
".\agent.exe" %*
set "agent_status=%errorlevel%"
popd
exit /b %agent_status%

:config
setlocal EnableDelayedExpansion
echo(!CD!\.env
endlocal
popd
exit /b 0

:help
echo JIsjtu                   Start the terminal interface
echo JIsjtu --web             Open the web interface
echo JIsjtu --tui             Start the terminal interface
echo JIsjtu --config          Show the configuration file path
echo JIsjtu --doctor          Check the configuration
echo JIsjtu --sso-login       Sign in to jAccount
echo JIsjtu --tool NAME [JSON] Run a tool directly
popd
exit /b 0
