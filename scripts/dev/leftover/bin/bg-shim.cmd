@echo off
rem A script on PATH that starts its work in the background and exits: the line that runs it shows no start /b.
start /b bun "%~dp0..\child.mjs" > NUL 2>&1
